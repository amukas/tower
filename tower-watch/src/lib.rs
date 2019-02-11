#[macro_use]
extern crate futures;
extern crate futures_watch;
extern crate tower_service;

use futures::{Async, Future, Poll, Stream};
use futures_watch::{Watch, WatchError as ExternalWatchError};
use std::{error::Error as StdError, fmt};
use tower_service::Service;

/// Binds new instances of a Service with a borrowed reference to the watched value.
pub trait Bind<T> {
    type Service;

    fn bind(&mut self, t: &T) -> Self::Service;
}

/// A Service that re-binds an inner Service each time a Watch is notified.
///
// This can be used to reconfigure Services from a shared or otherwise
// externally-controlled configuration source (for instance, a file system).
#[derive(Debug)]
pub struct WatchService<T, B: Bind<T>> {
    watch: Watch<T>,
    bind: B,
    inner: B::Service,
}

type Error = Box<StdError + Send + Sync>;

#[derive(Debug)]
struct WatchError(ExternalWatchError);

impl StdError for WatchError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self)
    }
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Watch error")
    }
}

#[derive(Debug)]
pub struct ResponseFuture<F>(F);

// ==== impl WatchService ====

impl<T, B: Bind<T>> WatchService<T, B> {
    /// Creates a new WatchService, bound from the initial value of `watch`.
    pub fn new(watch: Watch<T>, mut bind: B) -> WatchService<T, B> {
        let inner = bind.bind(&*watch.borrow());
        WatchService { watch, bind, inner }
    }

    /// Checks to see if the watch has been updated and, if so, bind the service.
    fn poll_rebind(&mut self) -> Poll<(), WatchError> {
        if try_ready!(self.watch.poll().map_err(WatchError)).is_some() {
            let t = self.watch.borrow();
            self.inner = self.bind.bind(&*t);
            Ok(().into())
        } else {
            // Will never be notified.
            Ok(Async::NotReady)
        }
    }
}

impl<T, B, Request> Service<Request> for WatchService<T, B>
where
    B: Bind<T>,
    <<B as Bind<T>>::Service as Service<Request>>::Error: Into<Error>,
    B::Service: Service<Request>,
{
    type Response = <B::Service as Service<Request>>::Response;
    type Error = Error;
    type Future = ResponseFuture<<B::Service as Service<Request>>::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        let _ = self.poll_rebind().map_err(|e: WatchError| Box::new(e))?;
        self.inner.poll_ready().map_err(|e| e.into())
    }

    fn call(&mut self, req: Request) -> Self::Future {
        ResponseFuture(self.inner.call(req))
    }
}

// ==== impl Bind<T> ====

impl<T, S, F> Bind<T> for F
where
    for<'t> F: FnMut(&'t T) -> S,
{
    type Service = S;

    fn bind(&mut self, t: &T) -> S {
        (self)(t)
    }
}

// ==== impl ResponseFuture ====

impl<F> Future for ResponseFuture<F>
where
    F: Future,
    F::Error: Into<Error>,
{
    type Item = F::Item;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.0.poll().map_err(|e| e.into())
    }
}

// ==== mod tests ====

#[cfg(test)]
mod tests {
    extern crate tokio;

    use super::*;
    use futures::future;

    #[test]
    fn rebind() {
        use tower_service::Service;
        struct Svc(usize);
        impl Service<()> for Svc {
            type Response = usize;
            type Error = Error;
            type Future = future::FutureResult<usize, Self::Error>;
            fn poll_ready(&mut self) -> Poll<(), Self::Error> {
                Ok(().into())
            }
            fn call(&mut self, _: ()) -> Self::Future {
                future::ok(self.0)
            }
        }

        let mut rt = tokio::runtime::current_thread::Runtime::new().unwrap();
        macro_rules! assert_ready {
            ($svc:expr) => {{
                let f = future::lazy(|| future::result($svc.poll_ready()));
                assert!(rt.block_on(f).expect("ready").is_ready(), "ready")
            }};
        }
        macro_rules! assert_call {
            ($svc:expr, $expect:expr) => {{
                let f = rt.block_on($svc.call(()));
                assert_eq!(f.expect("call"), $expect, "call")
            }};
        }

        let (watch, mut store) = Watch::new(1);
        let mut svc = WatchService::new(watch, |n: &usize| Svc(*n));

        assert_ready!(svc);
        assert_call!(svc, 1);

        assert_ready!(svc);
        assert_call!(svc, 1);

        store.store(2).expect("store");
        assert_ready!(svc);
        assert_call!(svc, 2);

        store.store(3).expect("store");
        store.store(4).expect("store");
        assert_ready!(svc);
        assert_call!(svc, 4);

        drop(store);
        assert_ready!(svc);
        assert_call!(svc, 4);
    }
}
