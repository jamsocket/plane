use self::{service::MakeProxyService, tls::TlsAcceptor};
use crate::{database::DroneDatabase, KeyCertPathPair, keys::CertifiedKey};
use anyhow::Result;
use hyper::{Server, server::conn::AddrIncoming};
use std::{net::SocketAddr, sync::Arc};

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
}

pub async fn serve(options: ProxyOptions) -> Result<()> {
    let make_proxy = MakeProxyService::new(options.db);
    
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
