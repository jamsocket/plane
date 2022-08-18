use anyhow::anyhow;
use anyhow::Result;
use dev::{
    resources::certs::Certificates,
    scratch_dir,
    timeout::{expect_to_stay_alive, LivenessGuard},
    util::random_loopback_ip,
};
use dis_spawner_drone::database_connection::DatabaseConnection;
use dis_spawner_drone::drone::proxy::ProxyOptions;
use http::StatusCode;
use integration_test::integration_test;
use reqwest::{Certificate, Client, ClientBuilder};
use std::{net::SocketAddr, time::Duration};
use tokio::time::Instant;

const CLUSTER: &str = "spawner.test";

struct Proxy {
    #[allow(unused)]
    guard: LivenessGuard,
    //ip: Ipv4Addr,
    bind_address: SocketAddr,
    certs: Certificates,
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
        };

        proxy.wait_ready().await?;
        Ok(proxy)
    }

    pub fn client(&self, subdomain: &str) -> Result<Client> {
        let cert = Certificate::from_pem(&self.certs.cert_pem.as_bytes()).unwrap();
        let client = ClientBuilder::new()
            .add_root_certificate(cert)
            .resolve(&format!("{}.{}", subdomain, CLUSTER), self.bind_address)
            .build()?;
        Ok(client)
    }
}

#[integration_test]
async fn backend_not_exist_404s() -> Result<()> {
    let proxy = Proxy::new().await?;

    let client = proxy.client("foobar")?;

    let result = client
        .get(format!(
            "https://foobar.spawner.test:{}/",
            proxy.bind_address.port()
        ))
        .send()
        .await;
    assert_eq!(StatusCode::NOT_FOUND, result.unwrap().status());

    Ok(())
}
