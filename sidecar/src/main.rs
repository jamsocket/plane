use hyper::service::make_service_fn;
use hyper::Server;
use proxy::ProxyService;
use std::{convert::Infallible, net::SocketAddr};

mod proxy;

#[tokio::main]
async fn main() {
    let addr = SocketAddr::from(([127, 0, 0, 1], 9090));

    let proxy = ProxyService::new("localhost:8080").expect("Couldn't construct proxy.");
    let make_svc = make_service_fn(|_conn| {
        let proxy = proxy.clone();
        async move { Ok::<_, Infallible>(proxy) }
    });

    let server = Server::bind(&addr).serve(make_svc);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
