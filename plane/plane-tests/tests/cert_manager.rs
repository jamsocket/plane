use crate::common::timeout::WithTimeout;
use common::test_env::TestEnvironment;
use plane::proxy::{
    cert_manager::watcher_manager_pair, proxy_connection::ProxyConnection,
    proxy_server::ProxyState, AcmeConfig, AcmeEabConfiguration,
};
use plane_client::names::{Name, ProxyName};
use plane_test_macro::plane_test;
use std::sync::Arc;

mod common;

#[plane_test]
async fn cert_manager_does_refresh(env: TestEnvironment) {
    let controller = env.controller().await;

    let dns = env.dns(&controller).await;
    tracing::info!("DNS: {:?}", dns.port);

    let pebble = env.pebble(dns.port).await;
    tracing::info!("Pebble: {}", pebble.directory_url);

    let acme_config = AcmeConfig {
        endpoint: pebble.directory_url.clone(),
        mailto_email: "test-cert@jamsocket.com".to_string(),
        accept_insecure_certs_for_testing: true,
        acme_eab_keypair: None,
    };

    let certs_dir = env.scratch_dir.join("certs");
    std::fs::create_dir_all(&certs_dir).unwrap();

    let (mut cert_watcher, cert_manager) = watcher_manager_pair(
        env.cluster.clone(),
        Some(&certs_dir.join("cert.json")),
        Some(acme_config.clone()),
    )
    .await
    .unwrap();

    let state = Arc::new(ProxyState::new(None));

    let _proxy_connection = ProxyConnection::new(
        ProxyName::new_random(),
        controller.client(),
        env.cluster.clone(),
        cert_manager,
        state.clone(),
    );
    cert_watcher
        .wait_for_initial_cert()
        .with_timeout(60)
        .await
        .unwrap()
        .unwrap();
}

#[plane_test(120)]
async fn cert_manager_does_refresh_eab(env: TestEnvironment) {
    let certs_dir = env.scratch_dir.join("certs");

    {
        let controller = env.controller().await;

        let dns = env.dns(&controller).await;

        let eab_keypair = AcmeEabConfiguration::new(
            "kid-1".to_string(),
            "zWNDZM6eQGHWpSRTPal5eIUYFTu7EajVIoguysqZ9wG44nMEtx3MUAsUDkMTQ12W".to_string(),
        )
        .unwrap();

        let pebble = env.pebble_with_eab(dns.port, eab_keypair.clone()).await;
        tracing::info!("Pebble: {}", pebble.directory_url);

        let acme_config = AcmeConfig {
            endpoint: pebble.directory_url.clone(),
            mailto_email: "test-cert@jamsocket.com".to_string(),
            accept_insecure_certs_for_testing: true,
            acme_eab_keypair: Some(eab_keypair),
        };

        std::fs::create_dir_all(&certs_dir).unwrap();

        let (mut cert_watcher, cert_manager) = watcher_manager_pair(
            env.cluster.clone(),
            Some(&certs_dir.join("cert.json")),
            Some(acme_config.clone()),
        )
        .await
        .unwrap();

        let state = Arc::new(ProxyState::new(None));

        let _proxy_connection = ProxyConnection::new(
            ProxyName::new_random(),
            controller.client(),
            env.cluster.clone(),
            cert_manager,
            state.clone(),
        );
        cert_watcher
            .wait_for_initial_cert()
            .with_timeout(60)
            .await
            .unwrap()
            .unwrap();
    }

    {
        let (mut cert_watcher, _cert_manager) = watcher_manager_pair(
            env.cluster.clone(),
            Some(&certs_dir.join("cert.json")),
            None, // No ACME config; force load from disk.
        )
        .await
        .unwrap();

        cert_watcher
            .wait_for_initial_cert()
            .with_timeout(60)
            .await
            .unwrap()
            .unwrap();
    }
}
