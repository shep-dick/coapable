use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use crate::message_types::{CoapRequest, CoapResponse};

/// A boxed future that is `Send` and `'static`. Defined inline to avoid
/// pulling in `futures-util` for a single type alias.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Upper-layer request handler. The transport layer calls this for every
/// deduplicated CON/NON request. Return `Some(response)` to send back,
/// or `None` to silently absorb (e.g. for observe deregistration via RST).
pub trait RequestHandler: Send + Sync + 'static {
    async fn handle(&self, request: CoapRequest, peer: SocketAddr) -> Option<CoapResponse>;
}

/// Security identity attached to a request by the security layer.
/// Initially only `None`; DTLS/OSCORE/ACE variants added in Phase 5-6
#[derive(Debug, Clone)]
pub enum SecurityContext {
    /// No security applied.
    None,
}

/// Type-erased extension map for middleware-injected data.
/// Analogous to `http::Extensions` — middleware inserts typed values,
/// handlers and downstream middleware retrieve them.
#[derive(Default)]
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Extensions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) {
        self.map.insert(TypeId::of::<T>(), Box::new(val));
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref())
    }

    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut())
    }

    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast().ok())
            .map(|boxed| *boxed)
    }
}

/// Request context that coap-lite does not provide.
pub struct RequestContext {
    /// Peer address the request was received from.
    pub peer: SocketAddr,
    /// Path parameters extracted by the router (e.g., ":id" -> "abc123").
    pub params: HashMap<String, String>,
    /// Security identity from the DTLS/OSCORE/ACE layer.
    pub security: SecurityContext,
    /// Type-erased extension map for middleware-injected data.
    pub extensions: Extensions,
}

impl RequestContext {
    pub fn new(peer: SocketAddr) -> Self {
        Self {
            peer,
            params: HashMap::new(),
            security: SecurityContext::None,
            extensions: Extensions::new(),
        }
    }
}

/// The bundle passed to handlers: the CoAP request plus routing/security context.
pub struct HandlerRequest {
    /// The CoAP request — use for method, path, payload, etc.
    pub request: CoapRequest,
    /// Routing and security context.
    pub ctx: RequestContext,
}

/// A type-erased, boxed handler function, generic over shared state `S`.
pub type BoxHandler<S> =
    Box<dyn Fn(HandlerRequest, Arc<S>) -> BoxFuture<CoapResponse> + Send + Sync>;

/// Conversion trait for turning async functions into [`BoxHandler`].
pub trait IntoHandler<S> {
    fn into_handler(self) -> BoxHandler<S>;
}

/// Blanket impl: any async function with the handler signature can be converted.
impl<F, Fut, S> IntoHandler<S> for F
where
    F: Fn(HandlerRequest, Arc<S>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CoapResponse> + Send + 'static,
    S: Send + Sync + 'static,
{
    fn into_handler(self) -> BoxHandler<S> {
        Box::new(move |req, state| Box::pin(self(req, state)))
    }
}
