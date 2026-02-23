use std::net::SocketAddr;

use coap_lite::CoapRequest;

use crate::transport::ClientInterface;

pub struct CoapClient {
    interface: ClientInterface,
}

impl CoapClient {
    pub async fn send_request(&self, req: CoapRequest<SocketAddr>, peer: SocketAddr) {
        self.interface
            .send_request(req.message, peer)
            .await
            .unwrap();
    }
}
