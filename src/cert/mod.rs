use self::https_client::get_https_client;
use crate::{keys::KeyCertPathPair, nats::TypedNats, messages::cert::SetAcmeDnsRecord};
use acme2::{
    gen_rsa_private_key, AccountBuilder, AuthorizationStatus, ChallengeStatus, Csr,
    DirectoryBuilder, OrderBuilder, OrderStatus,
};
use anyhow::{anyhow, Result};
use openssl::{
    pkey::{PKey, Private},
    x509::X509,
};
use reqwest::Client;
use std::time::Duration;

mod https_client;

const DNS_01: &str = "dns-01";

pub async fn get_certificate(
    cluster_domain: &str,
    nats: &TypedNats,
    acme_server_url: &str,
    client: &Client,
) -> Result<(PKey<Private>, X509)> {
    let _span = tracing::info_span!("Getting certificate", %cluster_domain);
    let _span_guard = _span.enter();

    let dir = DirectoryBuilder::new(acme_server_url.to_string())
        .http_client(client.clone())
        .build()
        .await?;

    let mut builder = AccountBuilder::new(dir);
    builder.contact(vec!["mailto:paul@driftingin.space".to_string()]);
    builder.terms_of_service_agreed(true);
    let account = builder.build().await?;

    let mut builder = OrderBuilder::new(account);
    builder.add_dns_identifier(format!("*.{}", cluster_domain));
    let order = builder.build().await?;

    let authorizations = order.authorizations().await?;
    for auth in authorizations {
        tracing::info!("Requesting challenge.");
        let challenge = auth
            .get_challenge(DNS_01)
            .ok_or_else(|| anyhow!("Couldn't obtain dns-01 challenge."))?;

        let value = challenge
            .key_authorization_encoded()?
            .ok_or_else(|| anyhow!("No authorization value."))?;

        tracing::info!("Requesting TXT record from platform.");
        nats.request(&SetAcmeDnsRecord::subject(), &SetAcmeDnsRecord {
            cluster: cluster_domain.to_string(),
            value,
        })
        .await?;

        tracing::info!("Validating challenge.");
        let challenge = challenge.validate().await?;
        let challenge = challenge.wait_done(Duration::from_secs(5), 3).await?;
        if challenge.status != ChallengeStatus::Valid {
            return Err(anyhow!("ACME challenge failed."));
        }

        tracing::info!("Validating authorization.");
        let authorization = auth.wait_done(Duration::from_secs(5), 3).await?;
        if authorization.status != AuthorizationStatus::Valid {
            return Err(anyhow!("ACME authorization failed."));
        }
    }

    tracing::info!("Waiting for order to become ready.");
    let order = order.wait_ready(Duration::from_secs(5), 3).await?;
    if order.status != OrderStatus::Ready {
        return Err(anyhow!("ACME order failed."));
    }

    tracing::info!("Waiting for order to become done.");
    let pkey = gen_rsa_private_key(4096)?;
    let order = order.finalize(Csr::Automatic(pkey.clone())).await?;
    let order = order.wait_done(Duration::from_secs(5), 3).await?;

    if order.status != OrderStatus::Valid {
        return Err(anyhow!("ACME order not valid."));
    }

    tracing::info!("Waiting for certificate.");
    let cert = order
        .certificate()
        .await?
        .ok_or_else(|| anyhow!("ACME order response didn't include certificate."))?;

    let cert = cert
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Certificate list is empty."))?;

    tracing::info!("Got certificate from ACME.");

    Ok((pkey, cert))
}

pub async fn refresh_certificate(
    cluster_domain: &str,
    nats_url: &str,
    key_paths: &KeyCertPathPair,
    acme_server_url: &str,
) -> Result<()> {
    let client = get_https_client()?;
    let nats = TypedNats::connect(nats_url).await?;

    let (pkey, cert) = get_certificate(cluster_domain, &nats, acme_server_url, &client).await?;

    std::fs::write(&key_paths.certificate_path, cert.to_pem()?)?;
    std::fs::write(
        &key_paths.private_key_path,
        pkey.private_key_to_pem_pkcs8()?,
    )?;

    Ok(())
}
