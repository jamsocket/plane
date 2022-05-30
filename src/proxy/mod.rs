use std::{
    convert::Infallible,
    future::{ready, Future, Ready},
    pin::Pin,
    task::Poll, net::SocketAddr,
};
use anyhow::Result;
use hyper::{Server};
use hyper::{service::Service, Body, Request, Response, StatusCode};

pub struct MakeProxyService {}

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
        ready(Ok(ProxyService {}))
    }
}

pub struct ProxyService {}

impl ProxyService {
    async fn handle(_req: Request<Body>) -> anyhow::Result<Response<Body>> {
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
        Box::pin(Self::handle(req))
    }
}

pub async fn serve(http_port: u16) -> Result<()> {
    let make_proxy = MakeProxyService {};

    //let incoming = AddrIncoming::bind(format!("127.0.0.1:{}", http_port).parse()?)?;
    //let server = Server::builder(incoming).serve(make_proxy);
    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let server = Server::bind(&addr).serve(make_proxy);

    server.await?;

    Ok(())
}