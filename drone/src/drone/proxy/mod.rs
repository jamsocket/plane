use self::{
    certs::CertRefresher, connection_tracker::ConnectionTracker, service::MakeProxyService,
    tls::TlsAcceptor,
};
use crate::{
    database::DroneDatabase, database_connection::DatabaseConnection, keys::KeyCertPathPair,
};
use anyhow::{Context, Result};
use hyper::{server::conn::AddrIncoming, Server};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::select;

mod certs;
mod connection_tracker;
mod service;
mod tls;

#[derive(PartialEq, Debug)]
pub struct ProxyOptions {
    pub db: DatabaseConnection,
    pub bind_address: SocketAddr,
    pub key_pair: Option<KeyCertPathPair>,
    pub cluster_domain: String,
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

async fn run_server(
    db: DroneDatabase,
    options: ProxyOptions,
    connection_tracker: ConnectionTracker,
) -> Result<()> {
    let make_proxy = MakeProxyService::new(db, options.cluster_domain, connection_tracker.clone());

    if let Some(key_pair) = options.key_pair {
        let cert_refresher = CertRefresher::new(key_pair.clone())
            .context("Error building cert refresher.")?;

        let tls_cfg = {
            let cfg = rustls::ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(cert_refresher.resolver()));

            Arc::new(cfg)
        };

        let incoming = AddrIncoming::bind(&options.bind_address).context("Error binding port for HTTPS.")?;
        let server = Server::builder(TlsAcceptor::new(tls_cfg, incoming)).serve(make_proxy);
        server.await.context("Error from TLS proxy.")?;
    } else {
        let server = Server::bind(&options.bind_address).serve(make_proxy);
        server.await.context("Error from non-TLS proxy.")?;
    }

    Ok(())
}

pub async fn serve(options: ProxyOptions) -> Result<()> {
    let connection_tracker = ConnectionTracker::default();
    let db = options.db.connection().await?;

    select! {
        result = run_server(db.clone(), options, connection_tracker.clone()) => {
            tracing::info!(?result, "run_server returned early.")
        }
        () = record_connections(db, connection_tracker) => {
            tracing::info!("record_connections returned early.")
        }
    };

    Ok(())
}
