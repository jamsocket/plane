use common::test_env::TestEnvironment;
use plane::{
    names::{AcmeDnsServerName, Name, ProxyName},
    protocol::{
        CertManagerRequest, CertManagerResponse, MessageFromDns, MessageFromProxy, MessageToDns,
        MessageToProxy,
    },
};
use plane_test_macro::plane_test;

mod common;

#[plane_test]
async fn dns_api(env: TestEnvironment) {
    let controller = env.controller().await;
    let mut proxy_client = controller
        .client()
        .proxy_connection(&env.cluster)
        .connect(&ProxyName::new_random())
        .await
        .unwrap();

    let mut dns_client = controller
        .client()
        .dns_connection()
        .connect(&AcmeDnsServerName::new_random())
        .await
        .unwrap();

    proxy_client
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::CertLeaseRequest,
        ))
        .await
        .unwrap();

    let MessageToProxy::CertManagerResponse(CertManagerResponse::CertLeaseResponse {
        accepted: true,
    }) = proxy_client.recv().await.unwrap()
    else {
        panic!("Expected CertLeaseResponse(true)");
    };

    proxy_client
        .send(MessageFromProxy::CertManagerRequest(
            CertManagerRequest::SetTxtRecord {
                txt_value: "foobaz".to_string(),
            },
        ))
        .await
        .unwrap();

    let MessageToProxy::CertManagerResponse(CertManagerResponse::SetTxtRecordResponse {
        accepted: true,
    }) = proxy_client.recv().await.unwrap()
    else {
        panic!("Expected SetTxtRecordResponse(true)");
    };

    dns_client
        .send(MessageFromDns::TxtRecordRequest {
            cluster: env.cluster.clone(),
        })
        .await
        .unwrap();

    let MessageToDns::TxtRecordResponse { cluster, txt_value } = dns_client.recv().await.unwrap();

    assert_eq!(cluster, env.cluster);
    assert_eq!(txt_value.as_deref(), Some("foobaz"));
}
