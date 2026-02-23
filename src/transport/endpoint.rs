use std::net::SocketAddr;

use super::{Result, TransportError};
use bytes::Bytes;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct CoapEndpoint {
    outbound_sender: mpsc::Sender<(Bytes, SocketAddr)>,
    inbound_receiver: mpsc::Receiver<(Bytes, SocketAddr)>,
    task: JoinHandle<Result<()>>,
}

impl CoapEndpoint {
    pub async fn start(socket: UdpSocket) -> Result<CoapEndpoint> {
        let (outbound_sender, mut outbound_receiver) = mpsc::channel::<(Bytes, SocketAddr)>(100);
        let (inbound_sender, inbound_receiver) = mpsc::channel::<(Bytes, SocketAddr)>(100);

        let task = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                tokio::select! {
                    res = socket.recv_from(&mut buf) => {
                        let (n, peer) = res?;
                        if inbound_sender.send((Bytes::copy_from_slice(&buf[..n]), peer)).await.is_err() {
                            return Ok(())
                        };
                    }
                    res = outbound_receiver.recv() => {
                        match res {
                            Some((pkt, peer)) => { socket.send_to(&pkt, peer).await?; }
                            None => return Ok(()),
                        }
                    }
                }
            }
        });

        Ok(CoapEndpoint {
            outbound_sender,
            inbound_receiver,
            task,
        })
    }

    pub async fn send_packet(&self, pkt: Bytes, peer: SocketAddr) -> Result<()> {
        self.outbound_sender.send((pkt, peer)).await?;
        Ok(())
    }

    pub async fn recv_packet(&mut self) -> Result<(Bytes, SocketAddr)> {
        self.inbound_receiver
            .recv()
            .await
            .ok_or(TransportError::Closed)
    }

    pub async fn shutdown(self) -> Result<()> {
        drop(self.outbound_sender);
        drop(self.inbound_receiver);
        self.task.await.map_err(|_| TransportError::Closed)?
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[tokio::test]
    async fn spawn_endpoint() {
        let sock = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        let endpoint = sock.local_addr().unwrap();

        let mut handle = CoapEndpoint::start(sock).await.unwrap();
        let sent_msg = "hello, world";

        handle
            .send_packet(Bytes::from_static(sent_msg.as_bytes()), endpoint)
            .await
            .unwrap();

        let (received_msg, _) = handle.recv_packet().await.unwrap();

        assert_eq!(
            String::from_str(sent_msg).unwrap(),
            String::from_utf8(received_msg.to_vec()).unwrap()
        );

        handle.shutdown().await.unwrap();
    }
}
