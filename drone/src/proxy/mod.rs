use self::{
    certs::CertRefresher, connection_tracker::ConnectionTracker, service::MakeProxyService,
    tls::TlsAcceptor,
};
use crate::{database::DroneDatabase, keys::KeyCertPaths};
use anyhow::{anyhow, Context};
use http::uri::Scheme;
use http::StatusCode;
use hyper::header::{HeaderValue, LOCATION};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{server::conn::AddrIncoming, Server};
use hyper::{Body, Request, Response, Uri};
use plane_core::NeverResult;
use std::net::SocketAddr;
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::select;

mod certs;
mod connection_tracker;
mod service;
mod tls;

pub const PLANE_AUTH_COOKIE: &str = "_plane_auth";

#[derive(Clone)]
pub struct ProxyOptions {
    pub db: DroneDatabase,
    pub bind_ip: IpAddr,
    pub bind_port: u16,
    pub key_pair: Option<KeyCertPaths>,
    pub bind_redir_port: Option<u16>,
    pub cluster_domain: String,
    pub passthrough: Option<SocketAddr>,
    pub allow_path_routing: bool,
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
        options.passthrough,
        options.allow_path_routing,
    );
    let bind_address = SocketAddr::new(options.bind_ip, options.bind_port);

    if let (Some(bind_redir_port), Some(key_pair)) = (options.bind_redir_port, options.key_pair) {
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
        let redirect_server = {
            let redirect_address = SocketAddr::new(options.bind_ip, bind_redir_port);
            let redirect_service = make_service_fn(|_socket: &AddrStream| async move {
                Ok::<_, anyhow::Error>(service_fn(move |req: Request<Body>| async move {
                    let mut redir_uri_parts = req.uri().clone().into_parts();
                    redir_uri_parts.scheme = Some(Scheme::HTTPS);
                    redir_uri_parts.authority = Some(
                        req.headers()
                            .get("host")
                            .ok_or_else(|| anyhow!("no host header"))?
                            .to_str()?
                            .parse()?,
                    );
                    let redir_uri = Uri::from_parts(redir_uri_parts)?;
                    let mut res = Response::new(Body::empty());
                    *res.status_mut() = StatusCode::PERMANENT_REDIRECT;
                    res.headers_mut()
                        .insert(LOCATION, HeaderValue::from_str(&redir_uri.to_string())?);

                    Ok::<_, anyhow::Error>(res)
                }))
            });
            Server::bind(&redirect_address).serve(redirect_service)
        };
        tokio::select! {
            err = server => err.context("Error from TLS proxy.")?,
            err = redirect_server => err.context("Error from HTTP redirect.")?
        }
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
