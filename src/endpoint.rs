use thiserror::Error;
use tokio::net::UdpSocket;

type Result<T> = std::result::Result<T, TransportError>;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("io error")]
    Io(#[from] std::io::Error),
}

pub struct CoapEndpoint {
    socket: UdpSocket,
}

impl CoapEndpoint {
    pub async fn new(addr: &str) -> Result<Self> {
        let socket = UdpSocket::bind(addr).await?;

        Ok(Self { socket })
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn udp_socket_constructor() {
        let sock = CoapEndpoint::new("0.0.0.0:0").await.unwrap();
    }
}
