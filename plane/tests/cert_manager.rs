use crate::common::resources::pebble::Pebble;
use crate::common::timeout::WithTimeout;
use common::test_env::TestEnvironment;
use plane::{
    names::{Name, ProxyName},
    proxy::{cert_manager::watcher_manager_pair, proxy_connection::ProxyConnection, AcmeConfig},
};
use plane_test_macro::plane_test;

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
        client: Pebble::client().unwrap(),
    };

    let certs_dir = env.scratch_dir.join("certs");
    std::fs::create_dir_all(&certs_dir).unwrap();

    let (mut cert_watcher, cert_manager) = watcher_manager_pair(
        env.cluster.clone(),
        Some(&certs_dir.join("cert.json")),
        Some(acme_config),
    )
    .unwrap();

    let _proxy_connection = ProxyConnection::new(
        ProxyName::new_random(),
        controller.client(),
        env.cluster.clone(),
        cert_manager,
    );
    let _cert = cert_watcher.next().with_timeout(60).await.unwrap().unwrap();
}
