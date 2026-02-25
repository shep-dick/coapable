use crate::client::ClientError;
use crate::server::ServerError;
use crate::transport::TransportError;

pub mod client;
pub mod message_types;
pub mod oscore;
pub mod server;
mod transport;

pub use client::{ClientRequestBuilder, CoapClient};
pub use message_types::{CoapRequest, CoapResponse};
pub use server::{CoapServer, HandlerRequest, RequestContext, Router, delete, get, post, put};
pub use transport::ClientInterface;
pub use transport::CoapEndpoint;
pub use transport::CoapStack;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error(transparent)]
    Client(#[from] ClientError),

    #[error(transparent)]
    Server(#[from] ServerError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_constructor() {
        let e = Error::Transport(TransportError::Io(std::io::Error::from_raw_os_error(22)));
        println!("error: {:?}", e);
    }
}
