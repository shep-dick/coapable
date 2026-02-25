use std::collections::HashMap;
use std::net::SocketAddr;

use bytes::Bytes;
use coap_lite::{MessageClass, MessageType, Packet};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

use crate::client::ClientRequest;
use crate::server::server::{ServerRequest, ServerResponse};
use crate::{CoapRequest, CoapResponse};

use super::endpoint::CoapEndpoint;
use super::exchange::Exchange;
use super::reliability::IDLE_SESSION_CLEANUP_INTERVAL_SECS;
use super::session::{AckResult, PeerSession};
use super::{Result, TransportError};

struct OutboundResponse {
    response: CoapResponse,
    peer: SocketAddr,
    token: Vec<u8>,
}

struct OutboundRequest {
    request: CoapRequest,
    peer: SocketAddr,
    response_tx: oneshot::Sender<std::result::Result<CoapResponse, TransportError>>,
}

#[derive(Clone)]
pub struct ClientInterface {
    outbound_sender: mpsc::Sender<OutboundRequest>,
}

impl ClientInterface {
    /// Send a CoAP request to a peer. Returns a receiver for the response.
    pub async fn send_request(
        &self,
        client_request: ClientRequest,
    ) -> Result<oneshot::Receiver<std::result::Result<CoapResponse, TransportError>>> {
        let r = client_request.request;

        let (response_tx, response_rx) =
            oneshot::channel::<std::result::Result<CoapResponse, TransportError>>();

        self.outbound_sender
            .send(OutboundRequest {
                request: r,
                peer: client_request.peer,
                response_tx,
            })
            .await
            .map_err(|_| TransportError::EndpointClosed)?;
        Ok(response_rx)
    }
}

pub struct ServerInterface {
    outbound_sender: mpsc::Sender<OutboundResponse>,
    server_request_receiver: mpsc::Receiver<ServerRequest>,
}

impl ServerInterface {
    /// Send a CoAP response to a peer (server-side).
    pub async fn send_response(&self, server_response: ServerResponse) -> Result<()> {
        self.outbound_sender
            .send(OutboundResponse {
                response: server_response.response,
                peer: server_response.peer,
                token: server_response.token,
            })
            .await
            .map_err(|_| TransportError::EndpointClosed)?;
        Ok(())
    }

    /// Receive the next inbound CoAP request (server-side).
    pub async fn recv_request(&mut self) -> Result<ServerRequest> {
        self.server_request_receiver
            .recv()
            .await
            .ok_or(TransportError::EndpointClosed)
    }
}

pub struct CoapStack {
    task: JoinHandle<Result<()>>,
}

impl CoapStack {
    /// Spawns context task and returns a lightweight handle for message passing.
    pub async fn start(
        mut endpoint: CoapEndpoint,
    ) -> Result<(ClientInterface, ServerInterface, Self)> {
        let (outbound_response_sender, mut outbound_response_receiver) =
            mpsc::channel::<OutboundResponse>(100);
        let (server_request_sender, server_request_receiver) = mpsc::channel::<ServerRequest>(100);
        let (outbound_request_sender, mut outbound_request_receiver) =
            mpsc::channel::<OutboundRequest>(100);

        let task = tokio::spawn(async move {
            let mut sessions: HashMap<SocketAddr, PeerSession> = HashMap::new();

            let mut next_cleanup =
                Instant::now() + Duration::from_secs(IDLE_SESSION_CLEANUP_INTERVAL_SECS);

            loop {
                // Compute the soonest retransmit deadline across all sessions.
                let next_deadline = sessions
                    .values()
                    .filter_map(|s| s.next_retransmit_deadline())
                    .min();

                let retransmission_timer = async {
                    match next_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending::<()>().await,
                    }
                };

                let cleanup_timer = tokio::time::sleep_until(next_cleanup);

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
                                            Bytes::copy_from_slice(&ack_bytes),
                                            peer,
                                        ).await?;
                                        session.cache_response_for_mid(mid, ack_bytes);
                                    }
                                }

                                // Dispatch by message code
                                match parsed.header.code {
                                    MessageClass::Request(_) => {
                                        let token = parsed.get_token().to_vec();
                                        if let Ok(request) = CoapRequest::from_packet(&parsed) {
                                            let _ = server_request_sender.send(ServerRequest { request, peer, token }).await;
                                        }
                                    }
                                    MessageClass::Response(_) => {
                                        // Separate response (CON or NON with same token)
                                        let token = parsed.get_token().to_vec();
                                        if let Ok(response) = CoapResponse::from_packet(&parsed) {
                                            session.complete_exchange(&token, response);
                                        }
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
                                                if let Ok(response) = CoapResponse::from_packet(&parsed) {
                                                    session.complete_exchange(&token, response);
                                                }
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

                    // Outbound Request: client wants to send request
                    res = outbound_request_receiver.recv() => {
                        match res {
                            Some(req) => {

                                let session = sessions
                                    .entry(req.peer)
                                    .or_insert_with(|| PeerSession::new(req.peer));

                                let token = session.allocate_token();
                                let mid = session.allocate_mid();


                                // @TODO: turn CoapRequest into coap_lite::Packet


                                let is_con = req.request.confirmable();

                                let pkt = req.request.to_packet(token.clone(), mid);

                                let bytes = match pkt.to_bytes() {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };

                                let exchange = if is_con {
                                    Exchange::new_con(
                                        token.clone(),
                                        req.peer,
                                        req.response_tx,
                                        mid,
                                        bytes.clone(),
                                        Instant::now(),
                                    )
                                } else {
                                    Exchange::new_non(token.clone(), req.peer, req.response_tx, Instant::now())
                                };

                                if is_con {
                                    session.insert_con_exchange(token, mid, exchange);
                                } else {
                                    session.insert_exchange(token, exchange);
                                }

                                endpoint.send_packet(Bytes::from(bytes), req.peer).await?;
                            }
                            None => {
                                return Ok(())
                            }
                        }

                    }

                    // Outbound Response: server wants to send repsonse
                    res = outbound_response_receiver.recv() => {
                        match res {
                            Some(resp) => {

                                let session = sessions
                                    .entry(resp.peer)
                                    .or_insert_with(|| PeerSession::new(resp.peer));

                                let mid = session.allocate_mid();

                                let pkt = resp.response.to_packet(resp.token, mid);

                                if let Ok(bytes) = pkt.to_bytes() {
                                    endpoint.send_packet(Bytes::from(bytes), resp.peer).await?;
                                }
                            }
                            None => return Ok(()),
                        }
                    }

                    // Retransmission Timer: retransmission and housekeeping
                    _ = retransmission_timer => {
                        let now = Instant::now();
                        let mut to_send: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
                        for session in sessions.values_mut() {
                            let retransmits = session.collect_retransmissions(now);
                            let peer = session.peer();
                            for bytes in retransmits {
                                to_send.push((peer, bytes));
                            }
                            session.evict_stale_dedup(now);
                        }
                        for (peer, bytes) in to_send {
                            endpoint.send_packet(Bytes::from(bytes), peer).await?;
                        }
                    }

                    // Session Idle Timeout
                    _ = cleanup_timer => {
                        sessions.retain(|_, session| !session.is_idle());
                        next_cleanup = Instant::now() + Duration::from_secs(IDLE_SESSION_CLEANUP_INTERVAL_SECS);
                    }
                }
            }
        });

        Ok((
            ClientInterface {
                outbound_sender: outbound_request_sender.clone(),
            },
            ServerInterface {
                outbound_sender: outbound_response_sender,
                server_request_receiver,
            },
            CoapStack { task },
        ))
    }

    pub async fn shutdown(self) -> Result<()> {
        self.task
            .await
            .map_err(|_| TransportError::EndpointClosed)?
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
        let (_client, mut server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Peer B sends a NON GET request to context A
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::NonConfirmable);
        request.set_token(vec![1, 2, 3, 4]);
        sock_b
            .send_to(&request.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let req = server.recv_request().await.unwrap();
        assert_eq!(req.peer, addr_b);
        assert!(matches!(req.request.method(), RequestType::Get));
        assert_eq!(req.token, &[1, 2, 3, 4]);

        drop(_client);
        drop(server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn demux_non_response_to_client() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Client sends a NON request to peer B
        let request = CoapRequest::new(RequestType::Get)
            .confirmable(false)
            .build();
        let client_request = ClientRequest {
            peer: addr_b,
            request,
            timeout: Duration::from_secs(5),
        };
        let response_rx = client.send_request(client_request).await.unwrap();

        // Peer B receives the request
        let mut buf = [0u8; 2048];
        let (n, from) = sock_b.recv_from(&mut buf).await.unwrap();
        assert_eq!(from, addr_a);
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        let token = received.get_token().to_vec();

        // Peer B sends a NON response with matching token
        let mut response = Packet::new();
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.header.set_type(MessageType::NonConfirmable);
        response.set_token(token);
        response.payload = b"hello".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = response_rx.await.unwrap().unwrap();
        assert_eq!(resp.status(), ResponseType::Content);
        assert_eq!(resp.payload(), b"hello");

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_request_piggybacked_response() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Client sends a CON request
        let request = CoapRequest::new(RequestType::Get).build();
        let client_request = ClientRequest {
            peer: addr_b,
            request,
            timeout: Duration::from_secs(5),
        };
        let response_rx = client.send_request(client_request).await.unwrap();

        // Peer B receives the CON request
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        let request_mid = received.header.message_id;
        let token = received.get_token().to_vec();

        // Peer B sends a piggybacked response (ACK with response code + same MID)
        let mut response = Packet::new();
        response.header.set_type(MessageType::Acknowledgement);
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.header.message_id = request_mid;
        response.set_token(token);
        response.payload = b"piggyback".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = response_rx.await.unwrap().unwrap();
        assert_eq!(resp.status(), ResponseType::Content);
        assert_eq!(resp.payload(), b"piggyback");

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_request_separate_response() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Client sends a CON request
        let request = CoapRequest::new(RequestType::Get).build();
        let client_request = ClientRequest {
            peer: addr_b,
            request,
            timeout: Duration::from_secs(5),
        };
        let response_rx = client.send_request(client_request).await.unwrap();

        // Peer B receives the CON request
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        let request_mid = received.header.message_id;
        let token = received.get_token().to_vec();

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
        response.set_token(token);
        response.payload = b"separate".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = response_rx.await.unwrap().unwrap();
        assert_eq!(resp.payload(), b"separate");

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_request_rst_fails_exchange() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Client sends a CON request
        let request = CoapRequest::new(RequestType::Get).build();
        let client_request = ClientRequest {
            peer: addr_b,
            request,
            timeout: Duration::from_secs(5),
        };
        let response_rx = client.send_request(client_request).await.unwrap();

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
        assert!(matches!(result, Err(TransportError::Reset)));

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn inbound_con_request_triggers_auto_ack() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (_client, mut server, stack) = CoapStack::start(endpoint).await.unwrap();

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
        let req = server.recv_request().await.unwrap();
        assert_eq!(req.peer, addr_b);
        assert_eq!(req.token, &[40, 41]);

        drop(_client);
        drop(server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dedup_drops_duplicate_non() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (_client, mut server, stack) = CoapStack::start(endpoint).await.unwrap();

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
        let req1 = server.recv_request().await.unwrap();
        assert_eq!(req1.token, &[50, 51]);

        let req2 = server.recv_request().await.unwrap();
        assert_eq!(req2.token, &[52, 53]);

        drop(_client);
        drop(server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn outbound_request_gets_mid_assigned() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Send two requests — they should get different MIDs
        let req1 = CoapRequest::new(RequestType::Get).confirmable(false).build();
        let _rx1 = client.send_request(ClientRequest {
            peer: addr_b,
            request: req1,
            timeout: Duration::from_secs(5),
        }).await.unwrap();

        let req2 = CoapRequest::new(RequestType::Get).confirmable(false).build();
        let _rx2 = client.send_request(ClientRequest {
            peer: addr_b,
            request: req2,
            timeout: Duration::from_secs(5),
        }).await.unwrap();

        let mut buf = [0u8; 2048];
        let (n1, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let p1 = Packet::from_bytes(&buf[..n1]).unwrap();

        let (n2, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let p2 = Packet::from_bytes(&buf[..n2]).unwrap();

        // MIDs should be different
        assert_ne!(p1.header.message_id, p2.header.message_id);
        // MIDs should be sequential (differ by 1)
        assert_eq!(p2.header.message_id, p1.header.message_id.wrapping_add(1));

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn con_request_timeout() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Client sends a CON request — peer never responds
        let request = CoapRequest::new(RequestType::Get).build();
        let client_request = ClientRequest {
            peer: addr_b,
            request,
            timeout: Duration::from_secs(5),
        };
        let response_rx = client.send_request(client_request).await.unwrap();

        // The exchange should eventually time out
        let result = response_rx.await.unwrap();
        assert!(matches!(result, Err(TransportError::Timeout { .. })));

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn non_request_timeout() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Client sends a NON request — peer never responds
        let request = CoapRequest::new(RequestType::Get).confirmable(false).build();
        let client_request = ClientRequest {
            peer: addr_b,
            request,
            timeout: Duration::from_secs(5),
        };
        let response_rx = client.send_request(client_request).await.unwrap();

        // The exchange should expire after NON_LIFETIME
        let result = response_rx.await.unwrap();
        assert!(matches!(
            result,
            Err(TransportError::Timeout { retransmits: 0 })
        ));

        drop(client);
        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_con_gets_cached_ack() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (_client, mut server, stack) = CoapStack::start(endpoint).await.unwrap();

        // Peer B sends a CON GET request
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::Confirmable);
        request.header.message_id = 5555;
        request.set_token(vec![80, 81]);
        let request_bytes = request.to_bytes().unwrap();

        sock_b.send_to(&request_bytes, addr_a).await.unwrap();

        // Peer B receives the auto-ACK
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let ack1 = Packet::from_bytes(&buf[..n]).unwrap();
        assert_eq!(ack1.header.get_type(), MessageType::Acknowledgement);
        assert_eq!(ack1.header.message_id, 5555);

        // Server receives the request
        let req = server.recv_request().await.unwrap();
        assert_eq!(req.token, &[80, 81]);

        // Peer B sends the same CON request again (duplicate)
        sock_b.send_to(&request_bytes, addr_a).await.unwrap();

        // Peer B should receive a second ACK with the same MID
        let (n2, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let ack2 = Packet::from_bytes(&buf[..n2]).unwrap();
        assert_eq!(ack2.header.get_type(), MessageType::Acknowledgement);
        assert_eq!(ack2.header.message_id, 5555);
        assert!(matches!(ack2.header.code, MessageClass::Empty));

        drop(_client);
        drop(server);
        stack.shutdown().await.unwrap();
    }
}
