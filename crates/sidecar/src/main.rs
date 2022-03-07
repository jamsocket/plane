use anyhow::anyhow;
use clap::Parser;
use hyper::service::make_service_fn;
use hyper::Server;
use dis_spawner_tracing::init_logging;
use proxy::ProxyService;
use std::{convert::Infallible, net::SocketAddr};

mod monitor;
mod network_status;
mod proxy;

#[derive(Parser)]
struct Opts {
    /// Port on localhost to proxy requests to. If not provided, the first TCP port
    /// listened on is used.
    #[clap(long, default_value = "8080")]
    upstream_port: u16,

    /// The port that this proxy server listens on.
    #[clap(long, default_value = "9090")]
    serve_port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let opts = Opts::parse();
    let addr = SocketAddr::from(([0, 0, 0, 0], opts.serve_port));
    tracing::info!(%opts.upstream_port, %opts.serve_port, "Proxy started.");

    let proxy = ProxyService::new(opts.upstream_port).map_err(|e| {
        anyhow!(
            "Couldn't construct proxy to {}. Error: {:?}",
            opts.upstream_port,
            e
        )
    })?;

    let make_svc = make_service_fn(|_conn| {
        let proxy = proxy.clone();
        async move { Ok::<_, Infallible>(proxy) }
    });

    let server = Server::bind(&addr).serve(make_svc);
    server.await?;

    Ok(())
}
