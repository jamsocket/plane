use anyhow::anyhow;
use anyhow::Result;
use dev::util::base_spawn_request;
use dev::{
    resources::certs::Certificates,
    resources::server::Server,
    scratch_dir,
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::random_loopback_ip,
};
use dis_spawner_drone::database::DroneDatabase;
use dis_spawner_drone::database_connection::DatabaseConnection;
use dis_spawner_drone::drone::proxy::ProxyOptions;
use http::StatusCode;
use integration_test::integration_test;
use reqwest::Response;
use reqwest::{Certificate, ClientBuilder};
use std::{net::SocketAddr, time::Duration};
use tokio::time::Instant;

const CLUSTER: &str = "spawner.test";

struct Proxy {
    #[allow(unused)]
    guard: LivenessGuard,
    //ip: Ipv4Addr,
    bind_address: SocketAddr,
    certs: Certificates,
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
        let certs = Certificates::new(
            "proxy",
            vec!["*.spawner.test".into(), "spawner.test".into()],
        )?;
        let bind_ip = random_loopback_ip();
        let bind_address = SocketAddr::new(bind_ip.into(), 4040);
        let db_connection = DatabaseConnection::new(
            scratch_dir("proxy")
                .join("drone.db")
                .to_str()
                .unwrap()
                .to_string(),
        );

        let db = db_connection.connection().await?;
        let options = ProxyOptions {
            db: db_connection,
            bind_address,
            key_pair: Some(certs.path_pair.clone()),
            cluster_domain: CLUSTER.into(),
        };
        let guard = expect_to_stay_alive(dis_spawner_drone::drone::proxy::serve(options));

        let proxy = Proxy {
            guard,
            bind_address,
            certs,
            db,
        };

        proxy.wait_ready().await?;
        Ok(proxy)
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
