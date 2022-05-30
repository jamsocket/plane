use crate::database::DroneDatabase;
use anyhow::Result;
use http::header::HeaderName;
use hyper::Server;
use hyper::{service::Service, Body, Request, Response, StatusCode};
use std::{
    convert::Infallible,
    future::{ready, Future, Ready},
    net::SocketAddr,
    pin::Pin,
    task::Poll,
};

pub struct MakeProxyService {
    db: DroneDatabase,
}

impl<T> Service<T> for MakeProxyService {
    type Response = ProxyService;
    type Error = Infallible;
    type Future = Ready<Result<ProxyService, Infallible>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: T) -> Self::Future {
        ready(Ok(ProxyService {
            db: self.db.clone(),
        }))
    }
}

pub struct ProxyService {
    db: DroneDatabase,
}

#[allow(unused)]
impl ProxyService {
    async fn handle(db: DroneDatabase, req: Request<Body>) -> anyhow::Result<Response<Body>> {
        if let Some(host) = req.headers().get(HeaderName::from_static("host")) {
            let host = std::str::from_utf8(&host.as_bytes())?;

            if let Some(addr) = db.get_proxy_route(host).await? {
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::empty())?);
            }
        }

        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())?)
    }
}

type ProxyServiceFuture =
    Pin<Box<dyn Future<Output = Result<Response<Body>, anyhow::Error>> + Send + 'static>>;

impl Service<Request<Body>> for ProxyService {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future = ProxyServiceFuture;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        Box::pin(Self::handle(self.db.clone(), req))
    }
}

pub async fn serve(db: DroneDatabase, http_port: u16) -> Result<()> {
    let make_proxy = MakeProxyService { db };
    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let server = Server::bind(&addr).serve(make_proxy);

    server.await?;

    Ok(())
}
