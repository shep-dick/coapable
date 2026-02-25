use std::net::SocketAddr;

use coap_lite::Packet;
use dashmap::DashMap;
use liboscore::PrimitiveContext;

use super::OscoreError;

pub struct OscoreContext {
    ctx: PrimitiveContext,
}

impl OscoreContext {
    fn protect_request(&mut self, pkt: Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    fn unprotect_request(&mut self, pkt: Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    fn protect_response(&mut self, pkt: Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    fn unprotect_response(&mut self, pkt: Packet) -> Result<Packet, OscoreError> {
        todo!()
    }
}

pub trait OscoreContextStore {
    fn insert(&mut self, peer: SocketAddr, ctx: OscoreContext);
    fn protect(&mut self, peer: SocketAddr, pkt: Packet) -> Result<Packet, OscoreError>;
    fn unprotect(&mut self, peer: SocketAddr, pkt: Packet) -> Result<Packet, OscoreError>;
}

pub struct DashmapOscoreContextStore {
    contexts: DashMap<SocketAddr, OscoreContext>,
}

impl DashmapOscoreContextStore {
    pub fn new() -> Self {
        Self {
            contexts: DashMap::new(),
        }
    }
}

impl OscoreContextStore for DashmapOscoreContextStore {
    fn insert(&mut self, peer: SocketAddr, ctx: OscoreContext) {
        // @TODO: we drop the old context silently if it already exists
        // maybe want to return it or include other safeguards
        let _ = self.contexts.insert(peer, ctx);
    }

    fn protect(&mut self, peer: SocketAddr, pkt: Packet) -> Result<Packet, OscoreError> {
        let ctx = self
            .contexts
            .get_mut(&peer)
            .ok_or(OscoreError::ContextNotFound)?
            .value_mut();

        todo!()
    }

    fn unprotect(&mut self, peer: SocketAddr, pkt: Packet) -> Result<Packet, OscoreError> {
        let ctx = self
            .contexts
            .get_mut(&peer)
            .ok_or(OscoreError::ContextNotFound)?
            .value_mut();

        todo!()
    }
}
