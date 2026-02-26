use std::net::SocketAddr;

use coap_lite::Packet;
use dashmap::DashMap;
use liboscore::{PrimitiveContext, raw::oscore_requestid_t};
use std::sync::Arc;

use super::OscoreError;

pub struct OscoreContext {
    ctx: PrimitiveContext,
    // Request ID dashmaps bind message token to request ID
    // Outbound request IDs are for client requests and responses
    // Inbound request IDs are for server requests and responses
    outbound_request_ids: DashMap<Vec<u8>, oscore_requestid_t>,
    inbound_request_ids: DashMap<Vec<u8>, oscore_requestid_t>,
}

// @TODO: I should eventually write my own OSCORE crypto + parsing business logic instead of using
// liboscore but for now, this will do

// SAFETY: PrimitiveContext is !Send only because it contains self-referential raw pointers
// (context.data -> primitive, primitive.immutables -> immutables). These pointers are
// reconstructed via fix() on every access through as_mut(), so they are never stale after
// a move. There is no C global state, TLS, or external shared references.
unsafe impl Send for OscoreContext {}
unsafe impl Sync for OscoreContext {}

// Actually I don't think we need to use a DashMap here
// We can just have this live only in the transport layer stack task
// In a repository in an application where we want to persist contexts we just store the immutables
// And we just re-initialize the contexts from the saved immutables on reboot

impl OscoreContext {
    pub fn protect_request(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    pub fn unprotect_request(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    pub fn protect_response(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        let request_id = self
            .inbound_request_ids
            .get(pkt.get_token())
            .ok_or(OscoreError::RequestNotFound)?
            .value();
        todo!()
    }

    pub fn unprotect_response(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        let request_id = self
            .outbound_request_ids
            .get(pkt.get_token())
            .ok_or(OscoreError::RequestNotFound)?
            .value();
        todo!()
    }
}

pub trait OscoreContextStore: Clone + Send + Sync {
    fn insert(&mut self, peer: SocketAddr, ctx: OscoreContext);
    fn with_context<F, T>(&self, peer: SocketAddr, f: F) -> Result<T, OscoreError>
    where
        F: FnOnce(&mut OscoreContext) -> Result<T, OscoreError>;
}

#[derive(Clone)]
pub struct DashmapOscoreContextStore {
    contexts: Arc<DashMap<SocketAddr, OscoreContext>>,
}

impl DashmapOscoreContextStore {
    pub fn new() -> Self {
        Self {
            contexts: Arc::new(DashMap::new()),
        }
    }
}

impl OscoreContextStore for DashmapOscoreContextStore {
    fn insert(&mut self, peer: SocketAddr, ctx: OscoreContext) {
        // @TODO: we drop the old context silently if it already exists
        // maybe want to return it or include other safeguards
        let _ = self.contexts.insert(peer, ctx);
    }

    fn with_context<F, T>(&self, peer: SocketAddr, f: F) -> Result<T, OscoreError>
    where
        F: FnOnce(&mut OscoreContext) -> Result<T, OscoreError>,
    {
        let mut ctx = self
            .contexts
            .get_mut(&peer)
            .ok_or(OscoreError::ContextNotFound)?;

        let res = f(ctx.value_mut());
        res
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn oscore_context_store_api() {
        let store = DashmapOscoreContextStore::new();

        let mut pkt = Packet::new();
        let protected = store
            .with_context("127.0.0.1:5683".parse().unwrap(), |ctx| {
                ctx.protect_request(&mut pkt)
            })
            .unwrap();
    }
}
