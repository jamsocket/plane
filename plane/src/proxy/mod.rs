use self::proxy_connection::ProxyConnection;
use crate::names::ProxyName;
use crate::proxy::cert_manager::watcher_manager_pair;
use crate::proxy::proxy_service::ProxyMakeService;
use crate::proxy::shutdown_signal::ShutdownSignal;
use crate::{client::PlaneClient, signals::wait_for_shutdown_signal, types::ClusterName};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use url::Url;

pub mod cert_manager;
mod cert_pair;
pub mod command;
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

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct ServerPortConfig {
    pub http_port: u16,
    pub https_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct ProxyPlan {
    pub name: ProxyName,
    pub controller_url: Url,
    pub cluster: ClusterName,
    pub cert_path: Option<PathBuf>,
    pub port_config: ServerPortConfig,
    pub acme_config: Option<AcmeConfig>,
    pub root_redirect_url: Option<Url>,
}

pub async fn run_proxy(plan: ProxyPlan) -> Result<()> {
    let client = PlaneClient::new(plan.controller_url);
    let (mut cert_watcher, cert_manager) = watcher_manager_pair(
        plan.cluster.clone(),
        plan.cert_path.as_deref(),
        plan.acme_config,
    )?;

    let proxy_connection = ProxyConnection::new(plan.name, client, plan.cluster, cert_manager);
    let shutdown_signal = ShutdownSignal::new();

    let https_redirect = plan.port_config.https_port.is_some();

    if plan.port_config.https_port.is_some() {
        cert_watcher.wait_for_initial_cert().await?;
    }

    let http_handle = ProxyMakeService {
        state: proxy_connection.state(),
        https_redirect,
        root_redirect_url: plan.root_redirect_url.clone(),
    }
    .serve_http(plan.port_config.http_port, shutdown_signal.subscribe())?;

    let https_handle = if let Some(https_port) = plan.port_config.https_port {
        tracing::info!("Waiting for initial certificate.");

        let https_handle = ProxyMakeService {
            state: proxy_connection.state(),
            https_redirect: false,
            root_redirect_url: plan.root_redirect_url,
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
