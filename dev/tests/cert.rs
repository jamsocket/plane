use anyhow::Result;
use integration_test::integration_test;
use openssl::x509::X509;
use plane_core::{messages::drone_state::DroneStateUpdate, nats::TypedNats, types::ClusterName};
use plane_dev::{
    resources::{nats::Nats, pebble::Pebble},
    scratch_dir,
    timeout::{spawn_timeout, timeout},
};
use plane_drone::{cert::acme::AcmeEabConfiguration, cert::CertOptions, keys::KeyCertPathPair};
use serde_json::Value;
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
        let handle = {
            // Old DNS message
            let expect_domain = ClusterName::new(expect_domain);
            let mut dns_sub = conn
                .subscribe(DroneStateUpdate::subscribe_subject_acme())
                .await?;
            spawn_timeout(10_000, "Should get ACME DNS request.", async move {
                let message = dns_sub.next().await.unwrap();
                let DroneStateUpdate::AcmeMessage(acme_message) = &message.value else {panic!()};
                assert_eq!(expect_domain, acme_message.cluster);
                message.respond(&Some(Value::Bool(true))).await?;
                Ok(())
            })
        };

        Ok(DummyDnsHandler { handle })
    }

    pub async fn finish(self) -> Result<()> {
        self.handle.await??;
        Ok(())
    }
}

#[integration_test]
async fn cert_refresh() {
    let nats = Nats::new().await.unwrap();
    let pebble = Pebble::new().await.unwrap();
    let conn = nats.connection().await.unwrap();
    let dns_handler = DummyDnsHandler::new(&conn, "plane.test").await.unwrap();

    let (_, certs) = timeout(
        60_000,
        "Getting certificate",
        plane_drone::cert::get_certificate(
            "plane.test",
            &conn,
            &pebble.directory_url(),
            "admin@plane.test",
            &pebble.client().unwrap(),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(2, certs.len());

    dns_handler.finish().await.unwrap();

    let alt_names = collect_alt_names(certs.first().unwrap());
    assert_eq!(vec!["[*.plane.test]".to_string()], alt_names);
}

#[integration_test]
async fn cert_refresh_full() {
    let nats = Nats::new().await.unwrap();
    let pebble = Pebble::new().await.unwrap();
    let conn = nats.connection().await.unwrap();
    let dns_handler = DummyDnsHandler::new(&conn, "plane.test").await.unwrap();
    let output_dir = scratch_dir("output");
    let key_paths = KeyCertPathPair {
        key_path: output_dir.join("output.key"),
        cert_path: output_dir.join("output.pem"),
    };

    timeout(
        60_000,
        "Getting certificate",
        plane_drone::cert::refresh_certificate(
            &CertOptions {
                cluster_domain: "plane.test".into(),
                nats: nats.connection().await.unwrap(),
                key_paths,
                email: "admin@plane.test".into(),
                acme_server_url: pebble.directory_url(),
                acme_eab_keypair: None,
            },
            &pebble.client().unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    dns_handler.finish().await.unwrap();
}

#[integration_test]
async fn cert_refresh_eab() {
    let eab_keypair = AcmeEabConfiguration::new(
        "kid-1",
        "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W",
    )
    .unwrap();

    let nats = Nats::new().await.unwrap();
    let pebble = Pebble::new_eab(&eab_keypair).await.unwrap();
    let conn = nats.connection().await.unwrap();
    let dns_handler = DummyDnsHandler::new(&conn, "plane.test").await.unwrap();

    let (_, certs) = timeout(
        60_000,
        "Getting certificate",
        plane_drone::cert::get_certificate(
            "plane.test",
            &conn,
            &pebble.directory_url(),
            "admin@plane.test",
            &pebble.client().unwrap(),
            Some(&eab_keypair),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    dns_handler.finish().await.unwrap();

    let alt_names = collect_alt_names(certs.first().unwrap());
    assert_eq!(vec!["[*.plane.test]".to_string()], alt_names);
}
