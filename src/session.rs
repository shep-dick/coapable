use std::collections::HashMap;
use std::net::SocketAddr;

use coap_lite::Packet;

use crate::exchange::Exchange;

pub struct PeerSession {
    peer: SocketAddr,
    exchanges: HashMap<Vec<u8>, Exchange>,
}

impl PeerSession {
    pub fn new(peer: SocketAddr) -> Self {
        Self {
            peer,
            exchanges: HashMap::new(),
        }
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    pub fn insert_exchange(&mut self, token: Vec<u8>, exchange: Exchange) {
        self.exchanges.insert(token, exchange);
    }

    /// Match an inbound response to an outstanding exchange by token.
    /// If found, delivers the response and removes the exchange.
    pub fn complete_exchange(&mut self, token: &[u8], response: Packet) -> bool {
        if let Some(exchange) = self.exchanges.remove(token) {
            exchange.complete(response);
            true
        } else {
            false
        }
    }
}
