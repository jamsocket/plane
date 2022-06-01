use self::{
    certs::CertRefresher, connection_tracker::ConnectionTracker, service::MakeProxyService,
    tls::TlsAcceptor,
};
use crate::{database::DroneDatabase, KeyCertPathPair};
use anyhow::Result;
use hyper::{server::conn::AddrIncoming, Server};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::select;

mod certs;
mod connection_tracker;
mod service;
mod tls;

pub struct ProxyHttpsOptions {
    pub port: u16,
    pub key_paths: KeyCertPathPair,
}

pub struct ProxyOptions {
    pub db: DroneDatabase,
    pub http_port: u16,
    pub https_options: Option<ProxyHttpsOptions>,
    pub cluster: String,
}

async fn record_connections(db: DroneDatabase, connection_tracker: ConnectionTracker) {
    loop {
        let backends = connection_tracker.get_and_clear_active_backends();
        if let Err(error) = db.reset_last_active_times(&backends).await {
            tracing::error!(?error, "Encountered database error.");
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn run_server(options: ProxyOptions, connection_tracker: ConnectionTracker) -> Result<()> {
    let make_proxy = MakeProxyService::new(options.db, options.cluster, connection_tracker.clone());

    if let Some(https_options) = options.https_options {
        let cert_refresher = CertRefresher::new(https_options.key_paths.clone())?;                

        let tls_cfg = {
            let cfg = rustls::ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(cert_refresher.resolver()));

            Arc::new(cfg)
        };

        let addr = SocketAddr::from(([0, 0, 0, 0], https_options.port));
        let incoming = AddrIncoming::bind(&addr)?;
        let server = Server::builder(TlsAcceptor::new(tls_cfg, incoming)).serve(make_proxy);
        server.await?;
    } else {
        let addr = SocketAddr::from(([0, 0, 0, 0], options.http_port));
        let server = Server::bind(&addr).serve(make_proxy);
        server.await?;
    }

    Ok(())
}

pub async fn serve(options: ProxyOptions) -> Result<()> {
    let connection_tracker = ConnectionTracker::default();
    let db = options.db.clone();

    select! {
        result = run_server(options, connection_tracker.clone()) => {
            tracing::info!(?result, "run_server returned early.")
        }
        () = record_connections(db, connection_tracker) => {
            tracing::info!("record_connections returned early.")
        }
    };

    Ok(())
}
