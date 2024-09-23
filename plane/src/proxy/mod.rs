use self::proxy_connection::ProxyConnection;
use crate::names::ProxyName;
use crate::proxy::cert_manager::watcher_manager_pair;
use crate::{client::PlaneClient, signals::wait_for_shutdown_signal, types::ClusterName};
use anyhow::Result;
use dynamic_proxy::server::{HttpsConfig, SimpleHttpServer};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;
use url::Url;

pub mod cert_manager;
mod cert_pair;
pub mod command;
mod connection_monitor;
pub mod proxy_connection;
// mod proxy_service;
mod proxy_server;
// mod rewriter;
mod request;
mod route_map;

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

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct ServerPortConfig {
    pub http_port: u16,
    pub https_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeEabConfiguration {
    /// The key identifier for the ACME EAB key.
    pub key_id: String,

    /// The HMAC key for the ACME EAB key, base64-encoded (URL, no-pad).
    pub key: String,
}

impl AcmeEabConfiguration {
    pub fn eab_key_b64(&self) -> String {
        self.key.clone()
    }

    pub fn new(key_id: String, key_b64: String) -> Result<Self> {
        let _ = data_encoding::BASE64URL_NOPAD.decode(key_b64.as_bytes())?;
        Ok(Self {
            key_id,
            key: key_b64,
        })
    }

    pub fn key_bytes(&self) -> Result<Vec<u8>> {
        Ok(data_encoding::BASE64URL_NOPAD.decode(self.key.as_bytes())?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeConfig {
    pub endpoint: Url,
    pub mailto_email: String,
    pub acme_eab_keypair: Option<AcmeEabConfiguration>,
    /// Don't validate the ACME server's certificate chain. This is ONLY for testing,
    /// and should not be used otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub accept_insecure_certs_for_testing: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub name: ProxyName,
    pub controller_url: Url,
    pub cluster: ClusterName,
    pub cert_path: Option<PathBuf>,
    pub port_config: ServerPortConfig,
    pub acme_config: Option<AcmeConfig>,
    pub root_redirect_url: Option<Url>,
}

pub async fn run_proxy(config: ProxyConfig) -> Result<()> {
    tracing::info!(name=%config.name, "Starting proxy");
    let client = PlaneClient::new(config.controller_url);
    let (mut cert_watcher, cert_manager) = watcher_manager_pair(
        config.cluster.clone(),
        config.cert_path.as_deref(),
        config.acme_config,
    )
    .await?;

    let proxy_connection = ProxyConnection::new(config.name, client, config.cluster, cert_manager);

    let https_redirect = config.port_config.https_port.is_some();

    if config.port_config.https_port.is_some() {
        cert_watcher.wait_for_initial_cert().await?;
    }

    let tcp_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        config.port_config.http_port,
    );
    let tcp_listener = TcpListener::bind(tcp_addr).await?;
    let http_server =
        SimpleHttpServer::new(proxy_connection.state(), tcp_listener, HttpsConfig::http())?;

    let https_server = if let Some(https_port) = config.port_config.https_port {
        tracing::info!("Waiting for initial certificate.");

        let tcp_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), https_port);
        let tcp_listener = TcpListener::bind(tcp_addr).await?;
        let https_server = SimpleHttpServer::new(
            proxy_connection.state(),
            tcp_listener,
            HttpsConfig::from_resolver(cert_watcher),
        )?;

        Some(https_server)
    } else {
        None
    };

    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down proxy server.");

    http_server
        .graceful_shutdown_with_timeout(Duration::from_secs(10))
        .await;
    if let Some(https_server) = https_server {
        https_server
            .graceful_shutdown_with_timeout(Duration::from_secs(10))
            .await;
    }

    Ok(())
}
