use self::proxy_connection::ProxyConnection;
use crate::names::ProxyName;
use crate::proxy::cert_manager::watcher_manager_pair;
use crate::{client::PlaneClient, signals::wait_for_shutdown_signal, types::ClusterName};
use anyhow::Result;
use plane_dynamic_proxy::server::{
    ServerWithHttpRedirect, ServerWithHttpRedirectConfig, ServerWithHttpRedirectHttpsConfig,
};
use proxy_server::ProxyState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub mod cert_manager;
mod cert_pair;
pub mod command;
pub mod connection_monitor;
pub mod proxy_connection;
pub mod proxy_server;
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
    /// The port to listen on for HTTP requests.
    /// If https_port is provided, this port will only serve a redirect to HTTPS.
    pub http_port: u16,

    /// The port to listen on for HTTPS requests.
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

    let state = Arc::new(ProxyState::new(
        config.root_redirect_url.map(|u| u.to_string()),
    ));

    // This returns a guard, we need to keep it in scope so that the connection is not terminated.
    let _proxy_connection = ProxyConnection::new(
        config.name,
        client,
        config.cluster,
        cert_manager,
        state.clone(),
    );

    let server = if let Some(https_port) = config.port_config.https_port {
        tracing::info!("Waiting for initial certificate.");
        cert_watcher.wait_for_initial_cert().await?;

        let https_config = ServerWithHttpRedirectHttpsConfig {
            https_port,
            resolver: Arc::new(cert_watcher),
        };

        let server_config = ServerWithHttpRedirectConfig {
            http_port: config.port_config.http_port,
            https_config: Some(https_config),
        };

        ServerWithHttpRedirect::new(state, server_config).await?
    } else {
        let server_config = ServerWithHttpRedirectConfig {
            http_port: config.port_config.http_port,
            https_config: None,
        };

        ServerWithHttpRedirect::new(state, server_config).await?
    };

    wait_for_shutdown_signal().await;
    tracing::info!("Shutting down proxy server.");

    server
        .graceful_shutdown_with_timeout(Duration::from_secs(10))
        .await;

    Ok(())
}
