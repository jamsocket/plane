use anyhow::Result;
use dev::{
    resources::{nats::Nats, pebble::Pebble},
    scratch_dir,
    timeout::{spawn_timeout, timeout},
};
use dis_spawner::{
    messages::cert::SetAcmeDnsRecord, nats::TypedNats, nats_connection::NatsConnection, types::ClusterName
};
use dis_spawner_drone::{
    drone::cli::{CertOptions, EabKeypair},
    keys::KeyCertPathPair,
};
use integration_test::integration_test;
use openssl::x509::X509;
use tokio::task::JoinHandle;

fn collect_alt_names(cert: &X509) -> Vec<String> {
    cert.subject_alt_names()
        .iter()
        .map(|d| format!("{:?}", d))
        .collect()
}

struct DummyDnsHandler {
    handle: JoinHandle<Result<()>>,
}

impl DummyDnsHandler {
    pub async fn new(conn: &TypedNats, expect_domain: &str) -> Result<DummyDnsHandler> {
        let expect_domain = ClusterName::new(expect_domain);
        let mut dns_sub = conn
            .subscribe(SetAcmeDnsRecord::subscribe_subject())
            .await?;
        let handle = spawn_timeout(10_000, "Should get ACME DNS request.", async move {
            let message = dns_sub.next().await.unwrap().unwrap();
            assert_eq!(expect_domain, message.value.cluster);
            message.respond(&true).await?;
            Ok(())
        });

        Ok(DummyDnsHandler { handle })
    }

    pub async fn finish(self) -> Result<()> {
        self.handle.await?
    }
}

#[integration_test]
async fn cert_refresh() -> Result<()> {
    let nats = Nats::new().await?;
    let pebble = Pebble::new().await?;
    let conn = nats.connection().await?;
    let dns_handler = DummyDnsHandler::new(&conn, "spawner.test").await?;

    let (_, certs) = timeout(
        60_000,
        "Getting certificate",
        dis_spawner_drone::drone::cert::get_certificate(
            "spawner.test",
            &conn,
            &pebble.directory_url(),
            "admin@spawner.test",
            &pebble.client()?,
            None,
        ),
    )
    .await?;

    assert_eq!(2, certs.len());

    dns_handler.finish().await?;

    let alt_names = collect_alt_names(&certs.first().unwrap());
    assert_eq!(vec!["[*.spawner.test]".to_string()], alt_names);

    Ok(())
}

#[integration_test]
async fn cert_refresh_full() -> Result<()> {
    let nats = Nats::new().await?;
    let pebble = Pebble::new().await?;
    let conn = nats.connection().await?;
    let dns_handler = DummyDnsHandler::new(&conn, "spawner.test").await?;
    let output_dir = scratch_dir("output");
    let key_paths = KeyCertPathPair {
        private_key_path: output_dir.join("output.key"),
        certificate_path: output_dir.join("output.pem"),
    };

    let () = timeout(
        60_000,
        "Getting certificate",
        dis_spawner_drone::drone::cert::refresh_certificate(
            &CertOptions {
                cluster_domain: "spawner.test".into(),
                nats: NatsConnection::new(nats.connection_string())?,
                key_paths,
                email: "admin@spawner.test".into(),
                acme_server_url: pebble.directory_url(),
                acme_eab_keypair: None,
            },
            &pebble.client()?,
        ),
    )
    .await?;

    dns_handler.finish().await?;

    Ok(())
}

#[integration_test]
async fn cert_refresh_eab() -> Result<()> {
    let eab_keypair = EabKeypair::new(
        "kid-1",
        "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W",
    )?;

    let nats = Nats::new().await?;
    let pebble = Pebble::new_eab(&eab_keypair).await?;
    let conn = nats.connection().await?;
    let dns_handler = DummyDnsHandler::new(&conn, "spawner.test").await?;

    let (_, certs) = timeout(
        60_000,
        "Getting certificate",
        dis_spawner_drone::drone::cert::get_certificate(
            "spawner.test",
            &conn,
            &pebble.directory_url(),
            "admin@spawner.test",
            &pebble.client()?,
            Some(&eab_keypair),
        ),
    )
    .await?;

    dns_handler.finish().await?;

    let alt_names = collect_alt_names(&certs.first().unwrap());
    assert_eq!(vec!["[*.spawner.test]".to_string()], alt_names);

    Ok(())
}
