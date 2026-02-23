use crate::transport::TransportError;

mod transport;

pub use transport::CoapContext;
pub use transport::CoapEndpoint;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Socket(#[from] TransportError),

    #[error("request timed out after {retransmits} retransmissions")]
    Timeout { retransmits: u32 },

    #[error("peer sent RST")]
    Reset,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_constructor() {
        let e = Error::Socket(TransportError::Io(std::io::Error::from_raw_os_error(22)));
        println!("error: {:?}", e);
    }
}
