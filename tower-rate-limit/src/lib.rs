//! Tower middleware that applies a timeout to requests.
//!
//! If the response does not complete within the specified timeout, the response
//! will be aborted.

#[macro_use]
extern crate futures;
extern crate tower_service;
extern crate tokio_timer;

use futures::{Future, Poll};
use tower_service::Service;
use tokio_timer::Delay;

use std::{error::Error, fmt};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RateLimit<T> {
    inner: T,
    rate: Rate,
    state: State,
}

#[derive(Debug, Copy, Clone)]
pub struct Rate {
    num: u64,
    per: Duration,
}

/// The request has been rate limited
///
/// TODO: Consider returning the original request
#[derive(Debug)]
struct RateLimitError;

impl Error for RateLimitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

impl fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "rate limit exceeded")
    }
}

pub struct ResponseFuture<T> {
    inner: Option<T>,
}

#[derive(Debug)]
enum State {
    // The service has hit its limit
    Limited(Delay),
    Ready {
        until: Instant,
        rem: u64,
    },
}

impl<T> RateLimit<T> {
    /// Create a new rate limiter
    pub fn new<Request>(inner: T, rate: Rate) -> Self
    where
        T: Service<Request>,
    {
        let state = State::Ready {
            until: Instant::now(),
            rem: rate.num,
        };

        RateLimit {
            inner,
            rate,
            state: state,
        }
    }

    /// Get a reference to the inner service
    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner service
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consume `self`, returning the inner service
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl Rate {
    /// Create a new rate
    ///
    /// # Panics
    ///
    /// This function panics if `num` or `per` is 0.
    pub fn new(num: u64, per: Duration) -> Self {
        assert!(num > 0);
        assert!(per > Duration::from_millis(0));

        Rate { num, per }
    }
}

impl<S, Request> Service<Request> for RateLimit<S>
where
    S: Service<Request>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = S::Response;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self.state {
            State::Ready { .. } => return Ok(().into()),
            State::Limited(ref mut sleep) => {
                let res = sleep.poll()
                    .map_err(|_| RateLimitError);

                try_ready!(res);
            }
        }

        self.state = State::Ready {
            until: Instant::now() + self.rate.per,
            rem: self.rate.num,
        };

        Ok(().into())
    }

    fn call(&mut self, request: Request) -> Self::Future {
        match self.state {
            State::Ready { mut until, mut rem } => {
                let now = Instant::now();

                // If the period has elapsed, reset it.
                if now >= until {
                    until = now + self.rate.per;
                    let rem = self.rate.num;

                    self.state = State::Ready { until, rem }
                }

                if rem > 1 {
                    rem -= 1;
                    self.state = State::Ready { until, rem };
                } else {
                    // The service is disabled until further notice
                    let sleep = Delay::new(until);
                    self.state = State::Limited(sleep);
                }

                // Call the inner future
                let inner = Some(self.inner.call(request));
                ResponseFuture { inner }
            }
            State::Limited(..) => {
                ResponseFuture { inner: None }
            }
        }
    }
}

impl<T> Future for ResponseFuture<T>
where
    T: Future,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = T::Item;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.inner {
            Some(ref mut f) => {
                f.poll().map_err(|e| e.into())
            }
            None => Err(RateLimitError.into())
        }
    }
}
