mod handler;
pub mod router;
pub mod server;

pub use router::{MethodRouter, Router, delete, get, post, put};
pub use server::{CoapServer, RequestContext};

#[derive(Debug, thiserror::Error)]
pub enum ServerError {}

#[allow(dead_code)]
type Result<T> = std::result::Result<T, ServerError>;
