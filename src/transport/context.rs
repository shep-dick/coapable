use std::collections::HashMap;
use std::net::SocketAddr;

use bytes::Bytes;
use coap_lite::{MessageClass, MessageType, Packet};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::TransportError;
use super::endpoint::CoapEndpoint;
use super::exchange::Exchange;
use super::session::{AckResult, PeerSession};
use crate::Error;

type Result<T> = std::result::Result<T, TransportError>;

/// An inbound CoAP request destined for server handlers.
pub type ServerRequest = (Packet, SocketAddr);

enum Outbound {
    /// Client sending a request; expects a response via oneshot.
    Request {
        packet: Packet,
        peer: SocketAddr,
        response_tx: oneshot::Sender<std::result::Result<Packet, Error>>,
    },
    /// Server sending a response.
    Response { packet: Packet, peer: SocketAddr },
}

pub struct CoapContext {
    outbound_sender: mpsc::Sender<Outbound>,
    server_request_receiver: mpsc::Receiver<ServerRequest>,
    task: JoinHandle<Result<()>>,
}

impl CoapContext {
    /// Spawns context task and returns a lightweight handle for message passing.
    pub async fn start(mut endpoint: CoapEndpoint) -> Result<CoapContext> {
        let (outbound_sender, mut outbound_receiver) = mpsc::channel::<Outbound>(100);
        let (server_request_sender, server_request_receiver) = mpsc::channel::<ServerRequest>(100);

        let task = tokio::spawn(async move {
            let mut sessions: HashMap<SocketAddr, PeerSession> = HashMap::new();

            loop {
                // Compute the soonest retransmit deadline across all sessions.
                let next_deadline = sessions
                    .values()
                    .filter_map(|s| s.next_retransmit_deadline())
                    .min();

                let timer = async {
                    match next_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending::<()>().await,
                    }
                };

                tokio::select! {
                    // Inbound: packet from network
                    res = endpoint.recv_packet() => {
                        let (raw, peer) = res?;

                        let parsed = match Packet::from_bytes(&raw) {
                            Ok(p) => p,
                            Err(_) => continue, // drop unparseable packets
                        };

                        let msg_type = parsed.header.get_type();
                        let mid = parsed.header.message_id;
                        let now = Instant::now();

                        let session = sessions
                            .entry(peer)
                            .or_insert_with(|| PeerSession::new(peer));

                        match msg_type {
                            MessageType::Confirmable | MessageType::NonConfirmable => {
                                // Dedup check
                                if session.is_duplicate(mid) {
                                    if msg_type == MessageType::Confirmable {
                                        // Re-send cached response if available
                                        if let Some(cached) = session.get_cached_response(mid) { // @TODO: what happens if we don't have a cached response?
                                            endpoint.send_packet(
                                                Bytes::from(cached.to_vec()),
                                                peer,
                                            ).await?;
                                        }
                                    }
                                    continue;
                                }
                                session.record_inbound_mid(mid, now);

                                // Auto-ACK for CON
                                if msg_type == MessageType::Confirmable { // @TODO: implement piggyback ACK response
                                    let mut ack = Packet::new();
                                    ack.header.set_type(MessageType::Acknowledgement);
                                    ack.header.code = MessageClass::Empty;
                                    ack.header.message_id = mid;
                                    if let Ok(ack_bytes) = ack.to_bytes() {
                                        endpoint.send_packet(
                                            Bytes::from(ack_bytes),
                                            peer,
                                        ).await?;
                                    }
                                }

                                // Dispatch by message code
                                match parsed.header.code {
                                    MessageClass::Request(_) => {
                                        if server_request_sender.send((parsed, peer)).await.is_err() {
                                            return Ok(());  // @TODO: don't want to tear down if server is dropped but client is still alive
                                        }
                                    }
                                    MessageClass::Response(_) => {
                                        // Separate response (CON or NON with same token)
                                        let token = parsed.get_token().to_vec();
                                        session.complete_exchange(&token, parsed);
                                    }
                                    MessageClass::Empty => {
                                        // Empty CON = CoAP ping (already auto-ACKed above)
                                    }
                                    _ => {} // Reserved
                                }
                            }
                            MessageType::Acknowledgement => {
                                match parsed.header.code {
                                    MessageClass::Empty => {
                                        // Empty ACK: cancel retransmission
                                        session.handle_ack(mid, &parsed);
                                    }
                                    MessageClass::Response(_) => {
                                        // Piggybacked response
                                        match session.handle_ack(mid, &parsed) {
                                            AckResult::PiggybackedResponse(token) => {
                                                session.complete_exchange(&token, parsed);
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {} // ACK with request code — ignore
                                }
                            }
                            MessageType::Reset => {
                                session.handle_rst(mid);
                            }
                        }
                    }

                    // Outbound: application wants to send
                    res = outbound_receiver.recv() => {
                        match res {
                            Some(Outbound::Request { mut packet, peer, response_tx }) => {
                                let session = sessions
                                    .entry(peer)
                                    .or_insert_with(|| PeerSession::new(peer));

                                let token = packet.get_token().to_vec();
                                let mid = session.allocate_mid();
                                packet.header.message_id = mid;

                                let is_con = packet.header.get_type() == MessageType::Confirmable;
                                let bytes = match packet.to_bytes() {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };

                                let exchange = if is_con {
                                    Exchange::new_con(
                                        token.clone(),
                                        peer,
                                        response_tx,
                                        mid,
                                        bytes.clone(),
                                        Instant::now(),
                                    )
                                } else {
                                    Exchange::new_non(token.clone(), peer, response_tx)
                                };

                                if is_con {
                                    session.insert_con_exchange(token, mid, exchange);
                                } else {
                                    session.insert_exchange(token, exchange);
                                }

                                endpoint.send_packet(Bytes::from(bytes), peer).await?;
                            }
                            Some(Outbound::Response { packet, peer }) => {
                                if let Ok(bytes) = packet.to_bytes() {
                                    endpoint.send_packet(Bytes::from(bytes), peer).await?;
                                }
                            }
                            None => return Ok(()),
                        }
                    }

                    // Timer: retransmission and housekeeping
                    _ = timer => {
                        let now = Instant::now();
                        for session in sessions.values_mut() {
                            let retransmits = session.collect_retransmissions(now);
                            let peer = session.peer();
                            for bytes in retransmits {
                                endpoint.send_packet(Bytes::from(bytes), peer).await?;
                            }
                            session.evict_stale_dedup(now);
                        }
                    }
                }
            }
        });

        Ok(CoapContext {
            outbound_sender,
            server_request_receiver,
            task,
        })
    }

    /// Send a CoAP request to a peer. Returns a receiver for the response.
    pub async fn send_request(
        &self,
        packet: Packet,
        peer: SocketAddr,
    ) -> Result<oneshot::Receiver<std::result::Result<Packet, Error>>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.outbound_sender
            .send(Outbound::Request {
                packet,
                peer,
                response_tx,
            })
            .await
            .map_err(|_| TransportError::Closed)?;
        Ok(response_rx)
    }

    /// Send a CoAP response to a peer (server-side).
    pub async fn send_response(&self, packet: Packet, peer: SocketAddr) -> Result<()> {
        self.outbound_sender
            .send(Outbound::Response { packet, peer })
            .await
            .map_err(|_| TransportError::Closed)?;
        Ok(())
    }

    /// Receive the next inbound CoAP request (server-side).
    pub async fn recv_request(&mut self) -> Result<ServerRequest> {
        self.server_request_receiver
            .recv()
            .await
            .ok_or(TransportError::Closed)
    }

    pub async fn shutdown(self) -> Result<()> {
        drop(self.outbound_sender);
        drop(self.server_request_receiver);
        self.task.await.map_err(|_| TransportError::Closed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coap_lite::{RequestType, ResponseType};
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn demux_non_request_to_server() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let mut ctx = CoapContext::start(endpoint).await.unwrap();

        // Peer B sends a NON GET request to context A
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::NonConfirmable);
        request.set_token(vec![1, 2, 3, 4]);
        sock_b
            .send_to(&request.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let (req, peer) = ctx.recv_request().await.unwrap();
        assert_eq!(peer, addr_b);
        assert!(matches!(
            req.header.code,
            MessageClass::Request(RequestType::Get)
        ));
        assert_eq!(req.get_token(), &[1, 2, 3, 4]);

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn demux_non_response_to_client() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Client sends a NON request to peer B
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::NonConfirmable);
        request.set_token(vec![5, 6, 7, 8]);
        let response_rx = ctx.send_request(request, addr_b).await.unwrap();

        // Peer B receives the request
        let mut buf = [0u8; 2048];
        let (n, from) = sock_b.recv_from(&mut buf).await.unwrap();
        assert_eq!(from, addr_a);
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        assert_eq!(received.get_token(), &[5, 6, 7, 8]);

        // Peer B sends a NON response with matching token
        let mut response = Packet::new();
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.header.set_type(MessageType::NonConfirmable);
        response.set_token(vec![5, 6, 7, 8]);
        response.payload = b"hello".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = response_rx.await.unwrap().unwrap();
        assert!(matches!(
            resp.header.code,
            MessageClass::Response(ResponseType::Content)
        ));
        assert_eq!(resp.payload, b"hello");

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_request_piggybacked_response() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Client sends a CON request
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::Confirmable);
        request.set_token(vec![10, 11]);
        let response_rx = ctx.send_request(request, addr_b).await.unwrap();

        // Peer B receives the CON request
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        let request_mid = received.header.message_id;
        assert_eq!(received.get_token(), &[10, 11]);

        // Peer B sends a piggybacked response (ACK with response code + same MID)
        let mut response = Packet::new();
        response.header.set_type(MessageType::Acknowledgement);
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.header.message_id = request_mid;
        response.set_token(vec![10, 11]);
        response.payload = b"piggyback".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = response_rx.await.unwrap().unwrap();
        assert!(matches!(
            resp.header.code,
            MessageClass::Response(ResponseType::Content)
        ));
        assert_eq!(resp.payload, b"piggyback");

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_request_separate_response() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Client sends a CON request
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::Confirmable);
        request.set_token(vec![20, 21]);
        let response_rx = ctx.send_request(request, addr_b).await.unwrap();

        // Peer B receives the CON request
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        let request_mid = received.header.message_id;

        // Peer B sends an empty ACK (acknowledges receipt, response pending)
        let mut ack = Packet::new();
        ack.header.set_type(MessageType::Acknowledgement);
        ack.header.code = MessageClass::Empty;
        ack.header.message_id = request_mid;
        sock_b
            .send_to(&ack.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        // Peer B later sends the actual response as a NON with same token
        let mut response = Packet::new();
        response.header.set_type(MessageType::NonConfirmable);
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.set_token(vec![20, 21]);
        response.payload = b"separate".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = response_rx.await.unwrap().unwrap();
        assert_eq!(resp.payload, b"separate");

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_request_rst_fails_exchange() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Client sends a CON request
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::Confirmable);
        request.set_token(vec![30, 31]);
        let response_rx = ctx.send_request(request, addr_b).await.unwrap();

        // Peer B receives the request
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        let request_mid = received.header.message_id;

        // Peer B sends RST
        let mut rst = Packet::new();
        rst.header.set_type(MessageType::Reset);
        rst.header.code = MessageClass::Empty;
        rst.header.message_id = request_mid;
        sock_b
            .send_to(&rst.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        // Client should get an error
        let result = response_rx.await.unwrap();
        assert!(matches!(result, Err(Error::Reset)));

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn inbound_con_request_triggers_auto_ack() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let mut ctx = CoapContext::start(endpoint).await.unwrap();

        // Peer B sends a CON GET request
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::Confirmable);
        request.header.message_id = 12345;
        request.set_token(vec![40, 41]);
        sock_b
            .send_to(&request.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        // Peer B should receive an auto-ACK
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let ack = Packet::from_bytes(&buf[..n]).unwrap();
        assert_eq!(ack.header.get_type(), MessageType::Acknowledgement);
        assert_eq!(ack.header.message_id, 12345);
        assert!(matches!(ack.header.code, MessageClass::Empty));

        // Server should also receive the request
        let (req, peer) = ctx.recv_request().await.unwrap();
        assert_eq!(peer, addr_b);
        assert_eq!(req.get_token(), &[40, 41]);

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dedup_drops_duplicate_non() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let mut ctx = CoapContext::start(endpoint).await.unwrap();

        // Peer B sends the same NON request twice (same MID)
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::NonConfirmable);
        request.header.message_id = 9999;
        request.set_token(vec![50, 51]);
        let bytes = request.to_bytes().unwrap();

        sock_b.send_to(&bytes, addr_a).await.unwrap();
        sock_b.send_to(&bytes, addr_a).await.unwrap();

        // Send a different request so we can verify only 2 arrive total
        let mut request2 = Packet::new();
        request2.header.code = MessageClass::Request(RequestType::Get);
        request2.header.set_type(MessageType::NonConfirmable);
        request2.header.message_id = 10000;
        request2.set_token(vec![52, 53]);
        sock_b
            .send_to(&request2.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        // Should receive only the first and the different one (not the duplicate)
        let (req1, _) = ctx.recv_request().await.unwrap();
        assert_eq!(req1.get_token(), &[50, 51]);

        let (req2, _) = ctx.recv_request().await.unwrap();
        assert_eq!(req2.get_token(), &[52, 53]);

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn outbound_request_gets_mid_assigned() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Send two requests — they should get different MIDs
        let mut req1 = Packet::new();
        req1.header.code = MessageClass::Request(RequestType::Get);
        req1.header.set_type(MessageType::NonConfirmable);
        req1.set_token(vec![60]);
        let _rx1 = ctx.send_request(req1, addr_b).await.unwrap();

        let mut req2 = Packet::new();
        req2.header.code = MessageClass::Request(RequestType::Get);
        req2.header.set_type(MessageType::NonConfirmable);
        req2.set_token(vec![61]);
        let _rx2 = ctx.send_request(req2, addr_b).await.unwrap();

        let mut buf = [0u8; 2048];
        let (n1, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let p1 = Packet::from_bytes(&buf[..n1]).unwrap();

        let (n2, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let p2 = Packet::from_bytes(&buf[..n2]).unwrap();

        // MIDs should be different
        assert_ne!(p1.header.message_id, p2.header.message_id);
        // MIDs should be sequential (differ by 1)
        assert_eq!(p2.header.message_id, p1.header.message_id.wrapping_add(1));

        ctx.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn con_request_timeout() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Client sends a CON request — peer never responds
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::Confirmable);
        request.set_token(vec![70, 71]);
        let response_rx = ctx.send_request(request, addr_b).await.unwrap();

        // The exchange should eventually time out
        let result = response_rx.await.unwrap();
        assert!(matches!(result, Err(Error::Timeout { .. })));

        ctx.shutdown().await.unwrap();
    }
}
