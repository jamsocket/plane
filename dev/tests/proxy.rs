use anyhow::anyhow;
use anyhow::Result;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use http::StatusCode;
use integration_test::integration_test;
use plane_core::messages::agent::BackendState;
use plane_core::NeverResult;
use plane_dev::{
    resources::certs::SelfSignedCert,
    resources::server::Server,
    scratch_dir,
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::base_spawn_request,
    util::random_loopback_ip,
};
use plane_drone::database::DroneDatabase;
use plane_drone::proxy::ProxyOptions;
use plane_drone::proxy::PLANE_AUTH_COOKIE;
use reqwest::redirect::Policy;
use reqwest::Response;
use reqwest::{Certificate, ClientBuilder};
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::{net::SocketAddr, time::Duration};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::{rustls::client as rustls_client, TlsConnector};
use tokio_tungstenite::{
    tungstenite::handshake::client::generate_key, tungstenite::protocol::Message, WebSocketStream,
};

const CLUSTER: &str = "plane.test";

struct Proxy {
    #[allow(unused)]
    guard: LivenessGuard<NeverResult>,
    bind_address: SocketAddr,
    bind_redir_address: SocketAddr,
    certs: SelfSignedCert,
    db: DroneDatabase,
}

impl Proxy {
    async fn wait_ready(&self) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10_000))
            .unwrap();

        loop {
            let cert = Certificate::from_pem(self.certs.cert_pem.as_bytes()).unwrap();
            let client = ClientBuilder::new()
                .add_root_certificate(cert)
                .resolve(CLUSTER, self.bind_address)
                .build()?;
            let url = format!("https://{}:{}/", CLUSTER, self.bind_address.port());
            let result = tokio::time::timeout_at(deadline, client.get(&url).send()).await;
            match result {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(_)) => (), // Not ready yet.
                Err(_) => return Err(anyhow!("Timed out before ready.")),
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn new(passthrough: Option<SocketAddr>) -> Result<Proxy> {
        let certs = SelfSignedCert::new("proxy", vec!["*.plane.test".into(), "plane.test".into()])?;
        let bind_ip = random_loopback_ip();
        let db = DroneDatabase::new(&scratch_dir("proxy").join("drone.db")).await?;

        let options = ProxyOptions {
            db: db.clone(),
            bind_ip: std::net::IpAddr::V4(bind_ip),
            bind_port: 4040,
            bind_redir_port: Some(9090),
            key_pair: Some(certs.path_pair.clone()),
            cluster_domain: CLUSTER.into(),
            passthrough,
            allow_path_routing: false,
        };
        let guard = expect_to_stay_alive(plane_drone::proxy::serve(options));

        let proxy = Proxy {
            guard,
            bind_address: SocketAddr::V4(SocketAddrV4::new(bind_ip, 4040)),
            bind_redir_address: SocketAddr::V4(SocketAddrV4::new(bind_ip, 9090)),
            certs,
            db,
        };

        proxy.wait_ready().await?;
        Ok(proxy)
    }

    pub fn update_cert(&mut self) -> Result<()> {
        let certs = SelfSignedCert::new("proxy", vec!["*.plane.test".into(), "plane.test".into()])?;
        self.certs = certs;
        Ok(())
    }

    pub async fn http_get(
        &self,
        subdomain: &str,
        path: &str,
        bearer_token: Option<&str>,
        cookie_token: Option<&str>,
    ) -> std::result::Result<Response, reqwest::Error> {
        let cert = Certificate::from_pem(self.certs.cert_pem.as_bytes()).unwrap();
        let hostname = format!("{}.{}", subdomain, CLUSTER);
        let client = ClientBuilder::new()
            .add_root_certificate(cert)
            .resolve(&hostname, self.bind_address)
            .cookie_store(true)
            .build()?;

        let path = if let Some(path) = path.strip_prefix('/') {
            path
        } else {
            path
        };

        let url = format!("https://{}:{}/{}", hostname, self.bind_address.port(), path);
        let mut req = client.get(url);

        if let Some(bearer_token) = bearer_token {
            req = req.bearer_auth(bearer_token);
        }

        if let Some(cookie_token) = cookie_token {
            req = req.header("Cookie", format!("{}={}", PLANE_AUTH_COOKIE, cookie_token));
        }

        req.send().await
    }

    pub async fn https_websocket(
        &self,
        subdomain: &str,
        path: &str,
    ) -> std::result::Result<
        (
            WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
            http::Response<std::option::Option<std::vec::Vec<u8>>>,
        ),
        tokio_tungstenite::tungstenite::Error,
    > {
        let hostname = format!("{}.{}", subdomain, CLUSTER);
        let path = if let Some(path) = path.strip_prefix('/') {
            path
        } else {
            path
        };

        let tcp_stream = TcpStream::connect(self.bind_address)
            .await
            .expect("failed to connect tcp");

        let mut root_certs = tokio_rustls::rustls::RootCertStore::empty();
        root_certs.add_parsable_certificates(
            &rustls_pemfile::certs(&mut self.certs.cert_pem.as_bytes()).unwrap(),
        );
        let client_config = Arc::new(
            tokio_rustls::rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_certs)
                .with_no_client_auth(),
        );

        let connector = TlsConnector::from(client_config);
        let domain =
            rustls_client::ServerName::try_from(hostname.as_str()).expect("invalid dns for tls");
        let tls_stream = connector
            .connect(domain, tcp_stream)
            .await
            .expect("could not finish tls handshake");

        let ws_adr = format!("ws://{}/{}", hostname, path);
        let req = hyper::Request::get(ws_adr)
            .header("Sec-WebSocket-Key", generate_key())
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Host", hostname)
            .body(())
            .unwrap();

        tokio_tungstenite::client_async(req, tls_stream).await
    }
}

#[integration_test]
async fn backend_not_exist_404s() {
    let proxy = Proxy::new(None).await.unwrap();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!(StatusCode::NOT_FOUND, result.status());
}

#[integration_test]
async fn backend_not_exist_passthrough() {
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let proxy = Proxy::new(Some(server.address)).await.unwrap();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!(StatusCode::OK, result.status());
}

#[integration_test]
async fn simple_backend_proxy() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    let hostname = format!("{}.{}", "foobar", CLUSTER);
    let client = ClientBuilder::new()
        .redirect(Policy::none())
        .resolve(&hostname, proxy.bind_redir_address)
        .build()
        .unwrap();

    let req = client.get(format!(
        "http://{}:{}{}",
        hostname,
        proxy.bind_redir_address.port(),
        "/testing/paths"
    ));
    let response = req.send().await.unwrap();

    assert_eq!(StatusCode::PERMANENT_REDIRECT, response.status());
    let mut https_uri = response.url().clone();
    https_uri.set_scheme("https").unwrap();
    assert_eq!(
        response.headers().get("LOCATION").unwrap(),
        https_uri.as_str()
    );

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string(), None)
        .await
        .unwrap();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!("Hello World", result.text().await.unwrap());
}

#[integration_test]
async fn simple_ws_backend_proxy() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::serve_web_sockets().await.unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string(), None)
        .await
        .unwrap();

    let (ws_stream, ws_handshake_resp) = proxy
        .https_websocket("foobar", "/")
        .await
        .expect("could not request ws from proxy");

    assert_eq!(
        ws_handshake_resp.status(),
        http::status::StatusCode::SWITCHING_PROTOCOLS
    );

    let (mut write, read) = StreamExt::split(ws_stream);

    let max_messages = 6;
    for i in 1..max_messages + 1 {
        write
            .send(Message::Text(format!("{}", i)))
            .await
            .expect("Failed to send message via proxy ws");
    }

    let read = tokio_stream::StreamExt::timeout(read, Duration::from_secs(10));

    let responses: Vec<String> = read
        .take(max_messages)
        .map(|x| {
            x.expect("response failed with timeout")
                .expect("failure while reading from stream")
                .to_string()
        })
        .collect()
        .await;

    assert_eq!(responses, ["1", "2", "3", "4", "5", "6"]);

    write.close().await.expect("Failed to close");
}

#[integration_test]
async fn connection_status_is_recorded() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();
    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string(), None)
        .await
        .unwrap();

    // Last active time is initially set to the time of creation.
    let t1_last_active = proxy
        .db
        .get_backend_last_active(&sr.backend_id)
        .await
        .unwrap();
    assert!(
        Utc::now().signed_duration_since(t1_last_active) < chrono::Duration::seconds(5),
        "Last active should be close to present."
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    proxy.http_get("foobar", "/", None, None).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let t2_last_active = proxy
        .db
        .get_backend_last_active(&sr.backend_id)
        .await
        .unwrap();
    assert!(
        t1_last_active < t2_last_active,
        "Last active should increase after activity."
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    let t3_last_active = proxy
        .db
        .get_backend_last_active(&sr.backend_id)
        .await
        .unwrap();
    assert_eq!(
        t3_last_active, t2_last_active,
        "Last active timestamp shouldn't change without new activity."
    );

    proxy.http_get("foobar", "/", None, None).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let t4_last_active = proxy
        .db
        .get_backend_last_active(&sr.backend_id)
        .await
        .unwrap();
    assert!(
        t3_last_active < t4_last_active,
        "Last active should increase after activity."
    );
}

#[integration_test]
async fn host_header_is_set() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|req| async move {
        req.headers()
            .get("host")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    })
    .await
    .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string(), None)
        .await
        .unwrap();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!("foobar.plane.test:4040", result.text().await.unwrap());
}

#[integration_test]
async fn update_certificates() {
    let mut proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    let original_cert = proxy.certs.cert_pem.clone();

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string(), None)
        .await
        .unwrap();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!("Hello World", result.text().await.unwrap());

    proxy.update_cert().unwrap();
    let new_cert = proxy.certs.cert_pem.clone();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!("Hello World", result.text().await.unwrap());

    // Ensure the certs are actually different.
    assert_ne!(original_cert, new_cert);
}

#[integration_test]
async fn simple_missing_bearer_token() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobar"),
        )
        .await
        .unwrap();

    let result = proxy.http_get("foobar", "/", None, None).await.unwrap();
    assert_eq!(StatusCode::UNAUTHORIZED, result.status());
}

#[integration_test]
async fn simple_bearer_token() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobar"),
        )
        .await
        .unwrap();

    let result = proxy
        .http_get("foobar", "/", Some("foobar"), None)
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, result.status());
}

#[integration_test]
async fn simple_wrong_bearer_token() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobaz"),
        )
        .await
        .unwrap();

    let result = proxy
        .http_get("foobar", "/", Some("foobar"), None)
        .await
        .unwrap();
    assert_eq!(StatusCode::FORBIDDEN, result.status());
}

#[integration_test]
async fn simple_cookie_bearer_token() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobar"),
        )
        .await
        .unwrap();

    let result = proxy
        .http_get("foobar", "/", None, Some("foobar"))
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, result.status());
}

#[integration_test]
async fn simple_cookie_set() {
    let proxy = Proxy::new(None).await.unwrap();
    let server = Server::new(|_| async { "Hello World".into() })
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobar"),
        )
        .await
        .unwrap();

    let result = proxy
        .http_get("foobar", "/_plane_auth?token=foobar", None, None)
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, result.status());
    assert_eq!("Hello World", result.text().await.unwrap());
}

#[integration_test]
async fn custom_path_cookie_set() {
    let proxy = Proxy::new(None).await.unwrap();
    let server =
        Server::new(
            |req| async move { format!("Hello World {}", req.uri().path_and_query().unwrap()) },
        )
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobar"),
        )
        .await
        .unwrap();

    let result = proxy
        .http_get(
            "foobar",
            "/_plane_auth?token=foobar&redirect=/blahblah%3Fa%3Db",
            None,
            None,
        ) // blahblah?a=b
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, result.status());
    assert_eq!("Hello World /blahblah?a=b", result.text().await.unwrap());
}

#[integration_test]
async fn custom_path_cookie_set_invalid_redirect() {
    let proxy = Proxy::new(None).await.unwrap();
    let server =
        Server::new(
            |req| async move { format!("Hello World {}", req.uri().path_and_query().unwrap()) },
        )
        .await
        .unwrap();

    let sr = base_spawn_request().await;
    proxy.db.insert_backend(&sr).await.unwrap();
    proxy
        .db
        .update_backend_state(&sr.backend_id, BackendState::Ready)
        .await
        .unwrap();

    proxy
        .db
        .insert_proxy_route(
            &sr.backend_id,
            "foobar",
            &server.address.to_string(),
            Some("foobar"),
        )
        .await
        .unwrap();

    let result = proxy
        .http_get(
            "foobar",
            "/_plane_auth?token=foobar&redirect=https://blahblah%3Fa%3Db",
            None,
            None,
        ) // blahblah?a=b
        .await
        .unwrap();

    assert_eq!(StatusCode::BAD_REQUEST, result.status());
    assert_eq!(
        "Redirect must be relative and start with a slash.",
        result.text().await.unwrap()
    );
}
