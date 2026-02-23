use std::collections::HashMap;
use std::net::SocketAddr;

use bytes::Bytes;
use coap_lite::{MessageClass, Packet};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::endpoint::{CoapEndpoint, TransportError};
use crate::exchange::Exchange;
use crate::session::PeerSession;

type Result<T> = std::result::Result<T, TransportError>;

/// An inbound CoAP request destined for server handlers.
pub type ServerRequest = (Packet, SocketAddr);

enum Outbound {
    /// Client sending a request; expects a response via oneshot.
    Request {
        packet: Packet,
        peer: SocketAddr,
        response_tx: oneshot::Sender<Packet>,
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
    pub async fn start(mut endpoint: CoapEndpoint) -> Result<CoapContext> {
        let (outbound_sender, mut outbound_receiver) = mpsc::channel::<Outbound>(100);
        let (server_request_sender, server_request_receiver) =
            mpsc::channel::<ServerRequest>(100);

        let task = tokio::spawn(async move {
            let mut sessions: HashMap<SocketAddr, PeerSession> = HashMap::new();

            loop {
                tokio::select! {
                    // Inbound: packet from network
                    res = endpoint.recv_packet() => {
                        let (raw, peer) = res?;

                        let parsed = match Packet::from_bytes(&raw) {
                            Ok(p) => p,
                            Err(_) => continue, // drop unparseable packets
                        };

                        let session = sessions
                            .entry(peer)
                            .or_insert_with(|| PeerSession::new(peer));

                        match parsed.header.code {
                            MessageClass::Request(_) => {
                                // Server-bound: deliver to request handler
                                if server_request_sender.send((parsed, peer)).await.is_err() {
                                    return Ok(());
                                }
                            }
                            MessageClass::Response(_) => {
                                // Client-bound: match to outstanding exchange by token
                                let token = parsed.get_token().to_vec();
                                session.complete_exchange(&token, parsed);
                            }
                            MessageClass::Empty => {
                                // ACK or RST — TODO: CON acknowledgment handling
                            }
                            _ => {} // Reserved codes, ignore
                        }
                    }

                    // Outbound: application wants to send
                    res = outbound_receiver.recv() => {
                        match res {
                            Some(Outbound::Request { packet, peer, response_tx }) => {
                                let session = sessions
                                    .entry(peer)
                                    .or_insert_with(|| PeerSession::new(peer));

                                let token = packet.get_token().to_vec();
                                let exchange = Exchange::new(
                                    token.clone(),
                                    peer,
                                    response_tx,
                                );
                                session.insert_exchange(token, exchange);

                                if let Ok(bytes) = packet.to_bytes() {
                                    endpoint.send_packet(Bytes::from(bytes), peer).await?;
                                }
                            }
                            Some(Outbound::Response { packet, peer }) => {
                                if let Ok(bytes) = packet.to_bytes() {
                                    endpoint.send_packet(Bytes::from(bytes), peer).await?;
                                }
                            }
                            None => return Ok(()),
                        }
                    }

                    // TODO: timer wheel tick for retransmissions
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
    ) -> Result<oneshot::Receiver<Packet>> {
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
    use coap_lite::{MessageType, RequestType, ResponseType};
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn demux_request_to_server() {
        // Context on socket A, raw peer on socket B
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let mut ctx = CoapContext::start(endpoint).await.unwrap();

        // Peer B sends a GET request to context A
        let mut request = Packet::new();
        request.header.code = MessageClass::Request(RequestType::Get);
        request.header.set_type(MessageType::NonConfirmable);
        request.set_token(vec![1, 2, 3, 4]);
        sock_b
            .send_to(&request.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        // Context delivers it as a server request
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
    async fn demux_response_to_client() {
        // Context on socket A, raw peer on socket B
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let ctx = CoapContext::start(endpoint).await.unwrap();

        // Client sends a request to peer B
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

        // Peer B sends a response with matching token
        let mut response = Packet::new();
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.header.set_type(MessageType::NonConfirmable);
        response.set_token(vec![5, 6, 7, 8]);
        response.payload = b"hello".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        // Client gets the response via oneshot
        let resp = response_rx.await.unwrap();
        assert!(matches!(
            resp.header.code,
            MessageClass::Response(ResponseType::Content)
        ));
        assert_eq!(resp.payload, b"hello");

        ctx.shutdown().await.unwrap();
    }
}