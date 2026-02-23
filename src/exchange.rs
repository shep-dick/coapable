use std::net::SocketAddr;

use coap_lite::Packet;
use tokio::sync::oneshot;

pub struct Exchange {
    token: Vec<u8>,
    peer: SocketAddr,
    response_tx: oneshot::Sender<Packet>,
}

impl Exchange {
    pub fn new(token: Vec<u8>, peer: SocketAddr, response_tx: oneshot::Sender<Packet>) -> Self {
        Self {
            token,
            peer,
            response_tx,
        }
    }

    pub fn token(&self) -> &[u8] {
        &self.token
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Deliver the response to the waiting caller. Consumes the exchange.
    pub fn complete(self, response: Packet) {
        let _ = self.response_tx.send(response);
    }
}
