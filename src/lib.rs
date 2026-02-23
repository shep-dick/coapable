use crate::client::ClientError;
use crate::server::ServerError;
use crate::transport::TransportError;

mod client;
mod server;
mod transport;

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
