mod context;
mod endpoint;
mod exchange;
mod reliability;
mod session;

pub use context::CoapContext;
pub use endpoint::CoapEndpoint;

use thiserror::Error;

type Result<T> = std::result::Result<T, TransportError>;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("io error")]
    Io(#[from] std::io::Error),

    #[error("packet sending error")]
    Send(#[from] tokio::sync::mpsc::error::SendError<(bytes::Bytes, std::net::SocketAddr)>),

    #[error("endpoint closed")]
    Closed,
}
