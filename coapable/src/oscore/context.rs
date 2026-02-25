use std::net::SocketAddr;

use coap_lite::Packet;
use dashmap::DashMap;
use liboscore::PrimitiveContext;
use std::sync::Arc;

use super::OscoreError;

pub struct OscoreContext {
    ctx: PrimitiveContext,
}

// @TODO: I should eventually write my own OSCORE en/decryption business logic instead of using
// liboscore. For now, this will do.

// SAFETY: PrimitiveContext is !Send only because it contains self-referential raw pointers
// (context.data -> primitive, primitive.immutables -> immutables). These pointers are
// reconstructed via fix() on every access through as_mut(), so they are never stale after
// a move. There is no C global state, TLS, or external shared references.
unsafe impl Send for OscoreContext {}
unsafe impl Sync for OscoreContext {}

impl OscoreContext {
    fn protect_request(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    fn unprotect_request(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    fn protect_response(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        todo!()
    }

    fn unprotect_response(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
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
