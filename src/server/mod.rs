mod server;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {}

type Result<T> = std::result::Result<T, ServerError>;
