use crate::network_status::wait_for_ready_port;
use anyhow::anyhow;
use clap::Parser;
use hyper::service::make_service_fn;
use hyper::Server;
use logging::init_logging;
use proxy::ProxyService;
use std::{convert::Infallible, net::SocketAddr};

mod logging;
mod monitor;
mod network_status;
mod proxy;

#[derive(Parser)]
struct Opts {
    #[clap(long)]
    upstream_port: Option<u16>,

    #[clap(long, default_value = "9090")]
    serve_port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let opts = Opts::parse();
    let port = wait_for_ready_port(opts.upstream_port).await?;
    let addr = SocketAddr::from(([0, 0, 0, 0], opts.serve_port));

    let upstream = format!("localhost:{}", port);
    tracing::info!(%upstream, %opts.serve_port, "Proxy started.");

    let proxy = ProxyService::new(&upstream)
        .map_err(|e| anyhow!("Couldn't construct proxy to {}. Error: {:?}", upstream, e))?;

    let make_svc = make_service_fn(|_conn| {
        let proxy = proxy.clone();
        async move { Ok::<_, Infallible>(proxy) }
    });

    let server = Server::bind(&addr).serve(make_svc);

    server.await?;

    Ok(())
}
