use std::future::Future;
use std::pin::Pin;

use crate::message_types::{CoapRequest, CoapResponse};

use super::server::RequestContext;

pub trait Handler: Send + Sync + 'static {
    fn call(
        &self,
        req: CoapRequest,
        ctx: RequestContext,
    ) -> Pin<Box<dyn Future<Output = CoapResponse> + Send + '_>>;
}

impl<F, Fut> Handler for F
where
    F: Fn(CoapRequest, RequestContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CoapResponse> + Send + 'static,
{
    fn call(
        &self,
        req: CoapRequest,
        ctx: RequestContext,
    ) -> Pin<Box<dyn Future<Output = CoapResponse> + Send + '_>> {
        Box::pin(self(req, ctx))
    }
}
