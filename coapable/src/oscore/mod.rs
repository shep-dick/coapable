pub mod codec;
pub mod context;

#[derive(Debug, thiserror::Error)]
pub enum OscoreError {
    #[error("no OSCORE context exists for peer")]
    ContextNotFound,

    #[error("no request id correlation exists for token")]
    RequestNotFound,

    #[error("OSCORE protect/unprotect operation failed")]
    ProtectFailed,

    #[error("failed to encode packet to wire format")]
    PacketEncodingFailed,

    #[error("failed to decode packet from wire format")]
    PacketDecodingFailed,
}
