pub mod context;

#[derive(Debug, thiserror::Error)]
pub enum OscoreError {
    #[error("no OSCORE context exists for peer")]
    ContextNotFound,

    #[error("no request id correlation exists for token")]
    RequestNotFound,
}
