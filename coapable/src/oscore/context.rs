use std::net::SocketAddr;

use coap_lite::Packet;
use dashmap::DashMap;
use liboscore::{PrimitiveContext, PrimitiveImmutables, raw::oscore_requestid_t};
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

// Actually I don't think we need it to be Send here
// We can just have this live only in the transport layer stack task
// In a repository in an application where we want to persist contexts we just store the immutables
// And we just re-initialize the contexts from the saved immutables on reboot

// Actually we do need it to be Send because of EDHOC... unless we make EDHOC a transport-layer process
// Or we make a channel for the EDHOC API functions to push new creds to the stack task
// Or we make stack check for EDHOC messages and extract the credentials after completion
// But then we'd need stack to own the EDHOC state machines as well (maybe not a bad thing?)
// Whatever for now we forge onwards

impl OscoreContext {
    pub fn new(ctx: PrimitiveContext) -> Self {
        Self {
            ctx,
            outbound_request_ids: DashMap::new(),
            inbound_request_ids: DashMap::new(),
        }
    }

    pub fn protect_request(&mut self, pkt: &Packet) -> Result<Packet, OscoreError> {
        let (protected, request_id) = super::codec::protect_request(pkt, &mut self.ctx)?;
        self.outbound_request_ids
            .insert(pkt.get_token().to_vec(), request_id);
        Ok(protected)
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
    fn add(&mut self, peer: SocketAddr, immutables: PrimitiveImmutables);
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
    fn add(&mut self, peer: SocketAddr, immutables: PrimitiveImmutables) {
        // @TODO: we drop the old context silently if it already exists
        // maybe want to return it or include other safeguards

        let ctx = PrimitiveContext::new_from_fresh_material(immutables);

        let _ = self.contexts.insert(peer, OscoreContext::new(ctx));
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

    use std::net::UdpSocket;

    use super::*;
    use coap_message::MinimalWritableMessage;
    use liboscore::{AeadAlg, HkdfAlg};

    #[test]
    fn oscore_context_store_api() {
        let mut store = DashmapOscoreContextStore::new();

        let peer: SocketAddr = "127.0.0.1:5683".parse().unwrap();

        // Using test vectors from RFC 8613 Appendix C.1.1
        let master_secret = b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10";
        let master_salt = b"\x9e\x7c\xa9\x22\x23\x78\x63\x40";
        let sender_id = b""; // Client has empty sender ID in this example
        let recipient_id = b"\x01"; // Server has ID 0x01

        let aead_alg = AeadAlg::from_number(10).unwrap();
        let hkdf_alg = HkdfAlg::from_number(5).unwrap();

        let immutables = PrimitiveImmutables::derive(
            hkdf_alg,
            &master_secret[..16],
            &master_salt[..8],
            None, // No ID context
            aead_alg,
            sender_id,
            recipient_id,
        )
        .unwrap();

        store.add(peer, immutables);

        let mut pkt = Packet::new();
        pkt.set_payload(b"hello oscore").unwrap();
        let protected = store
            .with_context("127.0.0.1:5683".parse().unwrap(), |ctx| {
                ctx.protect_request(&pkt)
            })
            .unwrap();

        let sock = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap()).unwrap();
        sock.send_to(&protected.to_bytes().unwrap(), peer).unwrap();
    }
}
