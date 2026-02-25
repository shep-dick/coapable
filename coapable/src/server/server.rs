use std::net::SocketAddr;

use crate::CoapResponse;
use crate::message_types::CoapRequest;
use crate::server::handler::RequestHandler;
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

pub struct CoapServer<S: Send + Sync + 'static> {
    interface: ServerInterface,
    router: Router<S>,
}

impl<S: Send + Sync + 'static> CoapServer<S> {
    pub fn new(interface: ServerInterface, router: Router<S>) -> Self {
        Self { interface, router }
    }

    pub async fn run(mut self) {
        loop {
            let req = match self.interface.recv_request().await {
                Ok(req) => req,
                Err(_) => break,
            };

            let token = req.token;
            let peer = req.peer;

            if let Some(response) = self.router.handle(req.request, peer).await {
                let resp = ServerResponse {
                    response,
                    peer,
                    token,
                };
                let _ = self.interface.send_response(resp).await;
            }
        }
    }
}
