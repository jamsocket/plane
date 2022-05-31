use self::{service::MakeProxyService, tls::TlsAcceptor, connection_tracker::ConnectionTracker};
use crate::{database::DroneDatabase, KeyCertPathPair, keys::CertifiedKey};
use anyhow::Result;
use hyper::{Server, server::conn::AddrIncoming};
use tokio::select;
use std::{net::SocketAddr, sync::Arc, time::Duration};

mod service;
mod tls;
mod connection_tracker;

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
        let cert_key_pair = CertifiedKey::new(&https_options.key_paths)?;

        let tls_cfg = {
            let cfg = rustls::ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_single_cert(
                    cert_key_pair.certificate.clone(),
                    cert_key_pair.private_key.clone(),
                )?;
    
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
            panic!("record_connections should never terminate.")
        }
    };

    Ok(())
}