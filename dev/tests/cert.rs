use anyhow::Result;
use integration_test::integration_test;
use openssl::x509::X509;
use plane_core::{
    messages::cert::SetAcmeDnsRecord,
    messages::dns::{DnsRecordType, SetDnsRecord},
    nats::TypedNats,
    types::ClusterName,
};
use plane_dev::{
    resources::{nats::Nats, pebble::Pebble},
    scratch_dir,
    timeout::{spawn_timeout, timeout},
};
use plane_drone::{cert::acme::AcmeEabConfiguration, cert::CertOptions, keys::KeyCertPathPair};
use tokio::task::JoinHandle;

fn collect_alt_names(cert: &X509) -> Vec<String> {
    cert.subject_alt_names()
        .iter()
        .map(|d| format!("{:?}", d))
        .collect()
}

struct DummyDnsHandler {
    new_handle: JoinHandle<Result<()>>,
    old_handle: JoinHandle<Result<()>>,
}

impl DummyDnsHandler {
    pub async fn new(conn: &TypedNats, expect_domain: &str) -> Result<DummyDnsHandler> {
        let old_handle = {
            // Old DNS message
            let expect_domain = ClusterName::new(expect_domain);
            let mut dns_sub = conn
                .subscribe(SetAcmeDnsRecord::subscribe_subject())
                .await?;
            spawn_timeout(10_000, "Should get ACME DNS request.", async move {
                let message = dns_sub.next().await.unwrap();
                assert_eq!(expect_domain, message.value.cluster);
                message.respond(&true).await?;
                Ok(())
            })
        };

        let new_handle = {
            // New DNS message
            let expect_domain = ClusterName::new(expect_domain);
            let mut dns_sub = conn.subscribe(SetDnsRecord::subscribe_subject()).await?;
            spawn_timeout(10_000, "Should get ACME DNS request.", async move {
                let message = dns_sub.next().await.unwrap();
                assert_eq!(expect_domain, message.value.cluster);
                assert_eq!("_acme-challenge", &message.value.name);
                assert_eq!(DnsRecordType::TXT, message.value.kind);
                Ok(())
            })
        };

        Ok(DummyDnsHandler { new_handle, old_handle })
    }

    pub async fn finish(self) -> Result<()> {
        self.new_handle.await??;
        self.old_handle.await??;
        Ok(())
    }
}

#[integration_test]
async fn cert_refresh() -> Result<()> {
    let nats = Nats::new().await?;
    let pebble = Pebble::new().await?;
    let conn = nats.connection().await?;
    let dns_handler = DummyDnsHandler::new(&conn, "plane.test").await?;

    let (_, certs) = timeout(
        60_000,
        "Getting certificate",
        plane_drone::cert::get_certificate(
            "plane.test",
            &conn,
            &pebble.directory_url(),
            "admin@plane.test",
            &pebble.client()?,
            None,
        ),
    )
    .await?;

    assert_eq!(2, certs.len());

    dns_handler.finish().await?;

    let alt_names = collect_alt_names(certs.first().unwrap());
    assert_eq!(vec!["[*.plane.test]".to_string()], alt_names);

    Ok(())
}

#[integration_test]
async fn cert_refresh_full() -> Result<()> {
    let nats = Nats::new().await?;
    let pebble = Pebble::new().await?;
    let conn = nats.connection().await?;
    let dns_handler = DummyDnsHandler::new(&conn, "plane.test").await?;
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
                nats: nats.connection().await?,
                key_paths,
                email: "admin@plane.test".into(),
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
    let eab_keypair = AcmeEabConfiguration::new(
        "kid-1",
        "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W",
    )?;

    let nats = Nats::new().await?;
    let pebble = Pebble::new_eab(&eab_keypair).await?;
    let conn = nats.connection().await?;
    let dns_handler = DummyDnsHandler::new(&conn, "plane.test").await?;

    let (_, certs) = timeout(
        60_000,
        "Getting certificate",
        plane_drone::cert::get_certificate(
            "plane.test",
            &conn,
            &pebble.directory_url(),
            "admin@plane.test",
            &pebble.client()?,
            Some(&eab_keypair),
        ),
    )
    .await?;

    dns_handler.finish().await?;

    let alt_names = collect_alt_names(certs.first().unwrap());
    assert_eq!(vec!["[*.plane.test]".to_string()], alt_names);

    Ok(())
}
