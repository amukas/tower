use crate::{Rate, RateLimit};
use crate::error::{Error, never::Never};
use tower_layer::Layer;
use tower_service::Service;
use std::time::Duration;

#[derive(Debug)]
pub struct RateLimitLayer {
    rate: Rate,
}

impl RateLimitLayer {
    pub fn new(num: u64, per: Duration) -> Self {
        let rate = Rate::new(num, per);
        RateLimitLayer { rate }
    }
}

impl<S, Request> Layer<S, Request> for RateLimitLayer
where
    S: Service<Request>,
    Error: From<S::Error>,
{
    type Response = S::Response;
    type Error = Error;
    type LayerError = Never;
    type Service = RateLimit<S>;

    fn layer(&self, service: S) -> Result<Self::Service, Self::LayerError> {
        Ok(RateLimit::new(service, self.rate))
    }
}
