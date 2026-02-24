use std::collections::HashMap;

use coap_lite::RequestType;

use super::handler::Handler;
use crate::message_types::{CoapRequest, CoapResponse};

pub struct MethodRouter {
    get: Option<Box<dyn Handler>>,
    post: Option<Box<dyn Handler>>,
    put: Option<Box<dyn Handler>>,
    delete: Option<Box<dyn Handler>>,
}

impl MethodRouter {
    fn new() -> Self {
        Self {
            get: None,
            post: None,
            put: None,
            delete: None,
        }
    }

    pub fn get<H: Handler>(mut self, handler: H) -> Self {
        self.get = Some(Box::new(handler));
        self
    }

    pub fn post<H: Handler>(mut self, handler: H) -> Self {
        self.post = Some(Box::new(handler));
        self
    }

    pub fn put<H: Handler>(mut self, handler: H) -> Self {
        self.put = Some(Box::new(handler));
        self
    }

    pub fn delete<H: Handler>(mut self, handler: H) -> Self {
        self.delete = Some(Box::new(handler));
        self
    }

    pub(crate) async fn dispatch(&self, req: CoapRequest) -> CoapResponse {
        let handler = match req.method() {
            RequestType::Get => self.get.as_ref(),
            RequestType::Post => self.post.as_ref(),
            RequestType::Put => self.put.as_ref(),
            RequestType::Delete => self.delete.as_ref(),
            _ => None,
        };

        match handler {
            Some(h) => h.call(req).await,
            None => CoapResponse::method_not_allowed(),
        }
    }

    fn merge(&mut self, other: MethodRouter) {
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

pub fn get<H: Handler>(handler: H) -> MethodRouter {
    MethodRouter::new().get(handler)
}

pub fn post<H: Handler>(handler: H) -> MethodRouter {
    MethodRouter::new().post(handler)
}

pub fn put<H: Handler>(handler: H) -> MethodRouter {
    MethodRouter::new().put(handler)
}

pub fn delete<H: Handler>(handler: H) -> MethodRouter {
    MethodRouter::new().delete(handler)
}

pub struct Router {
    routes: HashMap<String, MethodRouter>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    pub fn route(mut self, path: &str, method_router: MethodRouter) -> Self {
        let normalized = normalize_path(path);
        match self.routes.entry(normalized) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().merge(method_router);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(method_router);
            }
        }
        self
    }

    pub(crate) async fn dispatch(&self, req: CoapRequest) -> CoapResponse {
        let path = req.path();
        match self.routes.get(&path) {
            Some(method_router) => method_router.dispatch(req).await,
            None => CoapResponse::not_found(),
        }
    }
}

fn normalize_path(path: &str) -> String {
    let mut normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    if normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }

    normalized
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use coap_lite::{MessageClass, Packet, RequestType, ResponseType};

    use super::*;

    fn make_request(method: RequestType, path: &str) -> CoapRequest {
        let mut packet = Packet::new();
        packet.header.code = MessageClass::Request(method);
        for segment in path.trim_start_matches('/').split('/') {
            if !segment.is_empty() {
                packet.add_option(coap_lite::CoapOption::UriPath, segment.as_bytes().to_vec());
            }
        }
        let peer: SocketAddr = "127.0.0.1:5683".parse().unwrap();
        CoapRequest::from_raw(packet, peer)
    }

    #[tokio::test]
    async fn routes_get_request() {
        async fn handle_temp(_req: CoapRequest) -> CoapResponse {
            let mut packet = Packet::new();
            packet.header.code = MessageClass::Response(ResponseType::Content);
            packet.payload = b"22.5".to_vec();
            CoapResponse::from_packet(packet).unwrap()
        }

        let router = Router::new().route("/sensors/temp", get(handle_temp));

        let req = make_request(RequestType::Get, "/sensors/temp");
        let resp = router.dispatch(req).await;
        assert_eq!(resp.status(), ResponseType::Content);
        assert_eq!(resp.payload(), b"22.5");
    }

    #[tokio::test]
    async fn returns_not_found_for_unknown_path() {
        async fn handle(_req: CoapRequest) -> CoapResponse {
            CoapResponse::not_implemented()
        }

        let router = Router::new().route("/known", get(handle));

        let req = make_request(RequestType::Get, "/unknown");
        let resp = router.dispatch(req).await;
        assert_eq!(resp.status(), ResponseType::NotFound);
    }

    #[tokio::test]
    async fn returns_method_not_allowed() {
        async fn handle(_req: CoapRequest) -> CoapResponse {
            CoapResponse::not_implemented()
        }

        let router = Router::new().route("/resource", get(handle));

        let req = make_request(RequestType::Post, "/resource");
        let resp = router.dispatch(req).await;
        assert_eq!(resp.status(), ResponseType::MethodNotAllowed);
    }

    #[tokio::test]
    async fn chains_multiple_methods() {
        async fn handle_get(_req: CoapRequest) -> CoapResponse {
            let mut packet = Packet::new();
            packet.header.code = MessageClass::Response(ResponseType::Content);
            CoapResponse::from_packet(packet).unwrap()
        }

        async fn handle_post(_req: CoapRequest) -> CoapResponse {
            let mut packet = Packet::new();
            packet.header.code = MessageClass::Response(ResponseType::Created);
            CoapResponse::from_packet(packet).unwrap()
        }

        let router = Router::new().route("/lights", get(handle_get).post(handle_post));

        let req = make_request(RequestType::Get, "/lights");
        let resp = router.dispatch(req).await;
        assert_eq!(resp.status(), ResponseType::Content);

        let req = make_request(RequestType::Post, "/lights");
        let resp = router.dispatch(req).await;
        assert_eq!(resp.status(), ResponseType::Created);
    }
}
