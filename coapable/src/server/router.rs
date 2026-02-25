use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use coap_lite::{RequestType, ResponseType};

use crate::message_types::{CoapRequest, CoapResponse};

use super::handler::{
    BoxHandler, Extensions, HandlerRequest, IntoHandler, RequestContext, SecurityContext,
};

/// Per-method handler slots on a trie node.
struct MethodHandlers<S> {
    get: Option<BoxHandler<S>>,
    post: Option<BoxHandler<S>>,
    put: Option<BoxHandler<S>>,
    delete: Option<BoxHandler<S>>,
}

impl<S> MethodHandlers<S> {
    fn new() -> Self {
        Self {
            get: None,
            post: None,
            put: None,
            delete: None,
        }
    }

    fn retrieve_handler(&self, method: &RequestType) -> Option<&BoxHandler<S>> {
        match method {
            RequestType::Get => self.get.as_ref(),
            RequestType::Post => self.post.as_ref(),
            RequestType::Put => self.put.as_ref(),
            RequestType::Delete => self.delete.as_ref(),
            _ => None,
        }
    }

    fn has_any(&self) -> bool {
        self.get.is_some() || self.post.is_some() || self.put.is_some() || self.delete.is_some()
    }

    fn merge(&mut self, other: MethodHandlers<S>) {
        if other.get.is_some() {
            self.get = other.get;
        }
        if other.post.is_some() {
            self.post = other.post;
        }
        if other.put.is_some() {
            self.put = other.put;
        }
        if other.delete.is_some() {
            self.delete = other.delete;
        }
    }
}

/// A node in the routing trie.
struct Node<S> {
    /// Static path segment children.
    children: HashMap<Arc<str>, Node<S>>,
    /// Dynamic segment child (`:param_name`).
    param: Option<(Arc<str>, Box<Node<S>>)>,
    /// Handlers per CoAP method.
    handlers: MethodHandlers<S>,
}

impl<S> Node<S> {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            param: None,
            handlers: MethodHandlers::new(),
        }
    }
}

/// Builder for associating handlers with specific CoAP methods on a route.
pub struct MethodRouter<S> {
    handlers: MethodHandlers<S>,
}

impl<S: Send + Sync + 'static> MethodRouter<S> {
    pub fn get(mut self, handler: impl IntoHandler<S>) -> Self {
        self.handlers.get = Some(handler.into_handler());
        self
    }

    pub fn post(mut self, handler: impl IntoHandler<S>) -> Self {
        self.handlers.post = Some(handler.into_handler());
        self
    }

    pub fn put(mut self, handler: impl IntoHandler<S>) -> Self {
        self.handlers.put = Some(handler.into_handler());
        self
    }

    pub fn delete(mut self, handler: impl IntoHandler<S>) -> Self {
        self.handlers.delete = Some(handler.into_handler());
        self
    }
}

/// Create a [`MethodRouter`] with a GET handler.
pub fn get<S: Send + Sync + 'static>(handler: impl IntoHandler<S>) -> MethodRouter<S> {
    MethodRouter {
        handlers: MethodHandlers {
            get: Some(handler.into_handler()),
            ..MethodHandlers::new()
        },
    }
}

/// Create a [`MethodRouter`] with a POST handler.
pub fn post<S: Send + Sync + 'static>(handler: impl IntoHandler<S>) -> MethodRouter<S> {
    MethodRouter {
        handlers: MethodHandlers {
            post: Some(handler.into_handler()),
            ..MethodHandlers::new()
        },
    }
}

/// Create a [`MethodRouter`] with a PUT handler.
pub fn put<S: Send + Sync + 'static>(handler: impl IntoHandler<S>) -> MethodRouter<S> {
    MethodRouter {
        handlers: MethodHandlers {
            put: Some(handler.into_handler()),
            ..MethodHandlers::new()
        },
    }
}

/// Create a [`MethodRouter`] with a DELETE handler.
pub fn delete<S: Send + Sync + 'static>(handler: impl IntoHandler<S>) -> MethodRouter<S> {
    MethodRouter {
        handlers: MethodHandlers {
            delete: Some(handler.into_handler()),
            ..MethodHandlers::new()
        },
    }
}

/// Trie-based CoAP router with path parameter extraction and per-method dispatch.
pub struct Router<S: Send + Sync + 'static> {
    root: Node<S>,
    state: Arc<S>,
}

impl<S: Send + Sync + 'static> Router<S> {
    pub fn new(state: Arc<S>) -> Self {
        Self {
            root: Node::new(),
            state,
        }
    }

    /// Register handlers for a path. Path segments starting with `:` are
    /// dynamic parameters (e.g., `"devices/:id"`).
    pub fn route(mut self, path: &str, method_router: MethodRouter<S>) -> Self {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let node = Self::walk_or_create(&mut self.root, &segments);
        node.handlers.merge(method_router.handlers);
        self
    }

    fn walk_or_create<'a>(current: &'a mut Node<S>, segments: &[&str]) -> &'a mut Node<S> {
        let mut node = current;
        for &segment in segments {
            if let Some(param_name) = segment.strip_prefix(':') {
                let param_arc: Arc<str> = param_name.into();
                if node.param.is_none() {
                    node.param = Some((param_arc, Box::new(Node::new())));
                }
                node = &mut node.param.as_mut().unwrap().1;
            } else {
                let key: Arc<str> = segment.into();
                node = node.children.entry(key).or_insert_with(Node::new);
            }
        }
        node
    }

    /// Match a path against the trie. Returns the matched node and extracted
    /// path parameters, or `None` if no route matches.
    fn lookup(&self, segments: &[&str]) -> Option<(&Node<S>, HashMap<String, String>)> {
        let mut current = &self.root;
        let mut params = HashMap::new();

        for &segment in segments {
            // Static match takes priority over param match.
            if let Some(child) = current.children.get(segment) {
                current = child;
            } else if let Some((param_name, param_node)) = &current.param {
                params.insert(param_name.to_string(), segment.to_string());
                current = param_node;
            } else {
                return None;
            }
        }

        if current.handlers.has_any() {
            Some((current, params))
        } else {
            None
        }
    }
}

impl<S: Send + Sync + 'static> super::handler::RequestHandler for Router<S> {
    async fn handle(&self, request: CoapRequest, peer: SocketAddr) -> Option<CoapResponse> {
        let path = request.path();
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let method = request.method();

        // Trie lookup
        let (node, params) = match self.lookup(&segments) {
            Some(result) => result,
            None => {
                return Some(CoapResponse::new(ResponseType::NotFound).build());
            }
        };

        // Method dispatch
        let handler = match node.handlers.retrieve_handler(&method) {
            Some(h) => h,
            None => {
                return Some(CoapResponse::new(ResponseType::MethodNotAllowed).build());
            }
        };

        // Build HandlerRequest and invoke
        let handler_request = HandlerRequest {
            request,
            ctx: RequestContext {
                peer,
                params,
                security: SecurityContext::None,
                extensions: Extensions::new(),
            },
        };

        let response = handler(handler_request, self.state.clone()).await;
        Some(response)
    }
}
