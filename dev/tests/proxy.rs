use anyhow::anyhow;
use anyhow::Result;
use chrono::Utc;
use dev::util::base_spawn_request;
use dev::{
    resources::certs::SelfSignedCert,
    resources::server::Server,
    scratch_dir,
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::random_loopback_ip,
};
use dis_spawner::NeverResult;
use dis_spawner_drone::database::DroneDatabase;
use dis_spawner_drone::proxy::ProxyOptions;
use http::StatusCode;
use integration_test::integration_test;
use reqwest::Response;
use reqwest::{Certificate, ClientBuilder};
use std::net::SocketAddrV4;
use std::{net::SocketAddr, time::Duration};
use tokio::time::Instant;

const CLUSTER: &str = "spawner.test";

struct Proxy {
    #[allow(unused)]
    guard: LivenessGuard<NeverResult>,
    //ip: Ipv4Addr,
    bind_address: SocketAddr,
    certs: SelfSignedCert,
    db: DroneDatabase,
}

impl Proxy {
    async fn wait_ready(&self) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10_000))
            .unwrap();

        loop {
            let cert = Certificate::from_pem(&self.certs.cert_pem.as_bytes()).unwrap();
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

    pub async fn new() -> Result<Proxy> {
        let certs = SelfSignedCert::new(
            "proxy",
            vec!["*.spawner.test".into(), "spawner.test".into()],
        )?;
        let bind_ip = random_loopback_ip();
        let db = DroneDatabase::new(&scratch_dir("proxy").join("drone.db")).await?;

        let options = ProxyOptions {
            db: db.clone(),
            bind_ip: std::net::IpAddr::V4(bind_ip),
            bind_port: 4040,
            key_pair: Some(certs.path_pair.clone()),
            cluster_domain: CLUSTER.into(),
        };
        let guard = expect_to_stay_alive(dis_spawner_drone::proxy::serve(options));

        let proxy = Proxy {
            guard,
            bind_address: SocketAddr::V4(SocketAddrV4::new(bind_ip, 4040)),
            certs,
            db,
        };

        proxy.wait_ready().await?;
        Ok(proxy)
    }

    pub fn update_cert(&mut self) -> Result<()> {
        let certs = SelfSignedCert::new(
            "proxy",
            vec!["*.spawner.test".into(), "spawner.test".into()],
        )?;
        self.certs = certs;
        Ok(())
    }

    pub async fn http_get(
        &self,
        subdomain: &str,
        path: &str,
    ) -> std::result::Result<Response, reqwest::Error> {
        let cert = Certificate::from_pem(&self.certs.cert_pem.as_bytes()).unwrap();
        let hostname = format!("{}.{}", subdomain, CLUSTER);
        let client = ClientBuilder::new()
            .add_root_certificate(cert)
            .resolve(&hostname, self.bind_address)
            .build()?;

        let path = if let Some(path) = path.strip_prefix("/") {
            path
        } else {
            path
        };

        let url = format!("https://{}:{}/{}", hostname, self.bind_address.port(), path);
        client.get(url).send().await
    }
}

#[integration_test]
async fn backend_not_exist_404s() -> Result<()> {
    let proxy = Proxy::new().await?;

    let result = proxy.http_get("foobar", "/").await?;
    assert_eq!(StatusCode::NOT_FOUND, result.status());

    Ok(())
}

#[integration_test]
async fn simple_backend_proxy() -> Result<()> {
    let proxy = Proxy::new().await?;
    let server = Server::new(|_| async { "Hello World".into() }).await?;

    let sr = base_spawn_request();
    proxy.db.insert_backend(&sr).await?;

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string())
        .await?;

    let result = proxy.http_get("foobar", "/").await?;
    assert_eq!("Hello World", result.text().await?);

    Ok(())
}

#[integration_test]
async fn connection_status_is_recorded() -> Result<()> {
    let proxy = Proxy::new().await?;
    let server = Server::new(|_| async { "Hello World".into() }).await?;

    let sr = base_spawn_request();
    proxy.db.insert_backend(&sr).await?;
    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string())
        .await?;

    // Last active time is initially set to the time of creation.
    let t1_last_active = proxy.db.get_backend_last_active(&sr.backend_id).await?;
    assert!(
        Utc::now().signed_duration_since(t1_last_active) < chrono::Duration::seconds(5),
        "Last active should be close to present."
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    proxy.http_get("foobar", "/").await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let t2_last_active = proxy.db.get_backend_last_active(&sr.backend_id).await?;
    assert!(
        t1_last_active < t2_last_active,
        "Last active should increase after activity."
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    let t3_last_active = proxy.db.get_backend_last_active(&sr.backend_id).await?;
    assert_eq!(
        t3_last_active, t2_last_active,
        "Last active timestamp shouldn't change without new activity."
    );

    proxy.http_get("foobar", "/").await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let t4_last_active = proxy.db.get_backend_last_active(&sr.backend_id).await?;
    assert!(
        t3_last_active < t4_last_active,
        "Last active should increase after activity."
    );

    Ok(())
}

#[integration_test]
async fn host_header_is_set() -> Result<()> {
    let proxy = Proxy::new().await?;
    let server = Server::new(|req| async move {
        req.headers()
            .get("host")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    })
    .await?;

    let sr = base_spawn_request();
    proxy.db.insert_backend(&sr).await?;

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string())
        .await?;

    let result = proxy.http_get("foobar", "/").await?;
    assert_eq!("foobar.spawner.test:4040", result.text().await?);

    Ok(())
}

#[integration_test]
async fn update_certificates() -> Result<()> {
    let mut proxy = Proxy::new().await?;
    let server = Server::new(|_| async { "Hello World".into() }).await?;

    let sr = base_spawn_request();
    proxy.db.insert_backend(&sr).await?;

    let original_cert = proxy.certs.cert_pem.clone();

    proxy
        .db
        .insert_proxy_route(&sr.backend_id, "foobar", &server.address.to_string())
        .await?;

    let result = proxy.http_get("foobar", "/").await?;
    assert_eq!("Hello World", result.text().await?);

    proxy.update_cert()?;
    let new_cert = proxy.certs.cert_pem.clone();

    let result = proxy.http_get("foobar", "/").await?;
    assert_eq!("Hello World", result.text().await?);

    // Ensure the certs are actually different.
    assert_ne!(original_cert, new_cert);

    Ok(())
}
