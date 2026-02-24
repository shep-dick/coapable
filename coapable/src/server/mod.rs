mod handler;
pub mod router;
mod server;

pub use router::{get, post, put, delete, MethodRouter, Router};
pub use server::CoapServer;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {}

type Result<T> = std::result::Result<T, ServerError>;
