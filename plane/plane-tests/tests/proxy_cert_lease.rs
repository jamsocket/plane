use common::test_env::TestEnvironment;
use plane_common::{
    names::{Name, ProxyName},
    protocol::{CertManagerRequest, CertManagerResponse, MessageFromProxy, MessageToProxy},
};
use plane_test_macro::plane_test;

mod common;

#[plane_test]
async fn request_dns_lease(env: TestEnvironment) {
    let controller = env.controller().await;
    let mut client = controller
        .client()
        .proxy_connection(&env.cluster)
        .connect(&ProxyName::new_random())
        .await
        .unwrap();

    client
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::CertLeaseRequest,
        ))
        .unwrap();

    let MessageToProxy::CertManagerResponse(CertManagerResponse::CertLeaseResponse {
        accepted: true,
    }) = client.recv().await.unwrap()
    else {
        panic!("Expected CertLeaseResponse(true)");
    };
}

#[plane_test]
async fn request_dns_lease_fails_when_held(env: TestEnvironment) {
    let controller = env.controller().await;

    let mut client1 = controller
        .client()
        .proxy_connection(&env.cluster)
        .connect(&ProxyName::new_random())
        .await
        .unwrap();

    client1
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::CertLeaseRequest,
        ))
        .unwrap();

    let MessageToProxy::CertManagerResponse(CertManagerResponse::CertLeaseResponse {
        accepted: true,
    }) = client1.recv().await.unwrap()
    else {
        panic!("Expected CertLeaseResponse(true)");
    };

    let mut client2 = controller
        .client()
        .proxy_connection(&env.cluster)
        .connect(&ProxyName::new_random())
        .await
        .unwrap();

    client2
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::CertLeaseRequest,
        ))
        .unwrap();

    let MessageToProxy::CertManagerResponse(CertManagerResponse::CertLeaseResponse {
        accepted: false,
    }) = client2.recv().await.unwrap()
    else {
        panic!("Expected CertLeaseResponse(false) when lease is held.");
    };

    client1
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::ReleaseCertLease,
        ))
        .unwrap();

    // Avoid race.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    client2
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::CertLeaseRequest,
        ))
        .unwrap();

    let MessageToProxy::CertManagerResponse(CertManagerResponse::CertLeaseResponse {
        accepted: true,
    }) = client2.recv().await.unwrap()
    else {
        panic!("Expected CertLeaseResponse(true) when lease is released.");
    };
}
