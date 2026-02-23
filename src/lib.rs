use crate::endpoint::TransportError;

mod context;
mod endpoint;
mod exchange;
mod session;

pub use context::CoapContext;
pub use endpoint::CoapEndpoint;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Socket(#[from] TransportError),
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
