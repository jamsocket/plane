use anyhow::Result;
use dev::{
    resources::{nats::nats, pebble::pebble},
    timeout::{spawn_timeout, timeout},
};
use dis_spawner::messages::cert::SetAcmeDnsRecord;
use integration_test::integration_test;

#[integration_test]
async fn test_cert_refresh() -> Result<()> {
    let nats = nats().await?;
    let pebble = pebble().await?;
    let conn = nats.connection().await?;

    let mut dns_sub = conn.subscribe(SetAcmeDnsRecord::subject()).await?;
    let handle = spawn_timeout(10_000, async move {
        let message = dns_sub.next().await.unwrap().unwrap();
        assert_eq!("spawner.test", message.value.cluster);
        message.respond(&true).await?;
        Ok(())
    });

    let (_, cert) = timeout(
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

    handle.await???;

    let alt_names: Vec<String> = cert
        .subject_alt_names()
        .iter()
        .map(|d| format!("{:?}", d))
        .collect();

    assert_eq!(vec!["[*.spawner.test]".to_string()], alt_names);

    Ok(())
}
