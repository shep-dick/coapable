use coap_lite::MessageType;

use crate::message_types::CoapRequest;
use crate::transport::ServerInterface;

use super::router::Router;

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
            let (packet, peer) = match self.interface.recv_request().await {
                Ok(req) => req,
                Err(_) => break,
            };

            let token = packet.get_token().to_vec();
            let request = CoapRequest::from_raw(packet, peer);

            let response = self.router.dispatch(request).await;

            let mut response_packet = response.into_packet();
            response_packet.set_token(token);
            response_packet.header.set_type(MessageType::NonConfirmable);

            let _ = self.interface.send_response(response_packet, peer).await;
        }
    }
}
