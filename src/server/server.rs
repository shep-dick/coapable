use crate::transport::ServerInterface;
use coap_lite::ContentFormat;

use super::Result;

pub struct CoapServer {
    interface: ServerInterface,
}

impl CoapServer {
    async fn run(&mut self) {
        loop {
            let _ = self.interface.recv_request().await.unwrap();
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResourceMetadata {
    pub path: String,
    pub resource_type: Option<String>,
    pub interface: Option<String>,
    pub content_format: Option<ContentFormat>,
    pub observable: bool,
}
