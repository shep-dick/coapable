use std::net::SocketAddr;

use coap_lite::MessageType;

use crate::CoapResponse;
use crate::message_types::CoapRequest;
use crate::transport::ServerInterface;

use super::router::Router;

pub(crate) struct ServerRequest {
    pub request: CoapRequest,
    pub peer: SocketAddr,
    pub token: Vec<u8>,
}

pub(crate) struct ServerResponse {
    pub response: CoapResponse,
    pub peer: SocketAddr,
    pub token: Vec<u8>,
}

pub struct CoapServer {
    interface: ServerInterface,
    router: Router,
}

impl CoapServer {
    pub fn new(interface: ServerInterface, router: Router) -> Self {
        Self { interface, router }
    }

    pub async fn run(mut self) {
        loop {
            let req = match self.interface.recv_request().await {
                Ok(req) => req,
                Err(_) => break,
            };

            let token = req.token;
            let request = req.request;

            let response = self.router.dispatch(request).await;

            let resp = ServerResponse {
                response,
                peer: req.peer,
                token,
            };

            let _ = self.interface.send_response(resp).await;
        }
    }
}
