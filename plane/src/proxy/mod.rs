use self::proxy_connection::ProxyConnection;
use crate::names::ProxyName;
use crate::proxy::cert_manager::watcher_manager_pair;
use crate::proxy::proxy_service::ProxyMakeService;
use crate::proxy::shutdown_signal::ShutdownSignal;
use crate::{client::PlaneClient, signals::wait_for_shutdown_signal, types::ClusterName};
use anyhow::Result;
use std::net::IpAddr;
use std::path::Path;
use url::Url;

pub mod cert_manager;
mod cert_pair;
mod connection_monitor;
pub mod proxy_connection;
mod proxy_service;
mod rewriter;
mod route_map;
mod shutdown_signal;
mod tls;

#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Http,
    Https,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Https => "https",
        }
    }
}

/// Information about the incoming request that is forwarded to the request in
/// X-Forwarded-* headers.
#[derive(Debug, Clone, Copy)]
pub struct ForwardableRequestInfo {
    /// The IP address of the client that made the request.
    /// Forwarded as X-Forwarded-For.
    ip: IpAddr,

    /// The protocol of the incoming request.
    /// Forwarded as X-Forwarded-Proto.
    protocol: Protocol,
}

#[derive(Debug, Copy, Clone)]
pub struct ServerPortConfig {
    pub http_port: u16,
    pub https_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct AcmeEabConfiguration {
    pub key_id: String,
    pub key: Vec<u8>,
}

impl AcmeEabConfiguration {
    pub fn eab_key_b64(&self) -> String {
        data_encoding::BASE64URL_NOPAD.encode(&self.key)
    }

    pub fn new(key_id: &str, key_b64: &str) -> Result<Self> {
        let key = data_encoding::BASE64URL_NOPAD.decode(key_b64.as_bytes())?;
        Ok(Self {
            key_id: key_id.into(),
            key,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AcmeConfig {
    pub endpoint: Url,
    pub mailto_email: String,
    pub client: reqwest::Client,
    pub acme_eab_keypair: Option<AcmeEabConfiguration>,
}

pub async fn run_proxy(
    name: ProxyName,
    client: PlaneClient,
    cluster: ClusterName,
    cert_path: Option<&Path>,
    port_config: ServerPortConfig,
    acme_config: Option<AcmeConfig>,
    root_redirect_url: Option<Url>,
) -> Result<()> {
    let (mut cert_watcher, cert_manager) =
        watcher_manager_pair(cluster.clone(), cert_path, acme_config)?;

    let proxy_connection = ProxyConnection::new(name, client, cluster, cert_manager);
    let shutdown_signal = ShutdownSignal::new();

    let https_redirect = port_config.https_port.is_some();

    let http_handle = ProxyMakeService {
        state: proxy_connection.state(),
        https_redirect,
        root_redirect_url: root_redirect_url.clone(),
    }
    .serve_http(port_config.http_port, shutdown_signal.subscribe())?;

    let https_handle = if let Some(https_port) = port_config.https_port {
        tracing::info!("Waiting for initial certificate.");
        cert_watcher.wait_for_initial_cert().await?;

        let https_handle = ProxyMakeService {
            state: proxy_connection.state(),
            https_redirect: false,
            root_redirect_url,
        }
        .serve_https(https_port, cert_watcher, shutdown_signal.subscribe())?;

        Some(https_handle)
    } else {
        None
    };

    wait_for_shutdown_signal().await;
    shutdown_signal.shutdown();
    tracing::info!("Shutting down proxy server.");

    http_handle.await?;
    if let Some(https_handle) = https_handle {
        https_handle.await?;
    }

    Ok(())
}
