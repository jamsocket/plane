use self::{
    certs::CertRefresher, connection_tracker::ConnectionTracker, service::MakeProxyService,
    tls::TlsAcceptor,
};
use crate::{database::DroneDatabase, keys::KeyCertPathPair};
use anyhow::{anyhow, Context};
use dis_spawner::NeverResult;
use hyper::{server::conn::AddrIncoming, Server};
use std::net::SocketAddr;
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::select;

mod certs;
mod connection_tracker;
mod service;
mod tls;

pub struct ProxyOptions {
    pub db: DroneDatabase,
    pub bind_ip: IpAddr,
    pub bind_port: u16,
    pub key_pair: Option<KeyCertPathPair>,
    pub cluster_domain: String,
}

async fn record_connections(
    db: DroneDatabase,
    connection_tracker: ConnectionTracker,
) -> NeverResult {
    loop {
        let backends = connection_tracker.get_and_clear_active_backends();
        if let Err(error) = db.reset_last_active_times(&backends).await {
            tracing::error!(?error, "Encountered database error.");
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn run_server(options: ProxyOptions, connection_tracker: ConnectionTracker) -> NeverResult {
    let make_proxy = MakeProxyService::new(
        options.db,
        options.cluster_domain,
        connection_tracker.clone(),
    );
    let bind_address = SocketAddr::new(options.bind_ip, options.bind_port);

    if let Some(key_pair) = options.key_pair {
        let cert_refresher =
            CertRefresher::new(key_pair.clone()).context("Error building cert refresher.")?;

        let tls_cfg = {
            let cfg = rustls::ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(cert_refresher.resolver()));

            Arc::new(cfg)
        };

        let incoming =
            AddrIncoming::bind(&bind_address).context("Error binding port for HTTPS.")?;
        let server = Server::builder(TlsAcceptor::new(tls_cfg, incoming)).serve(make_proxy);
        server.await.context("Error from TLS proxy.")?;
    } else {
        let server = Server::bind(&bind_address).serve(make_proxy);
        server.await.context("Error from non-TLS proxy.")?;
    };

    Err(anyhow!("Server should not have terminated, but did."))
}

pub async fn serve(options: ProxyOptions) -> NeverResult {
    let connection_tracker = ConnectionTracker::default();

    select! {
        result = record_connections(options.db.clone(), connection_tracker.clone()) => {
            tracing::info!("record_connections returned early.");
            result
        }
        result = run_server(options, connection_tracker) => {
            tracing::info!(?result, "run_server returned early.");
            result
        }
    }
}
