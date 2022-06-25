use self::https_client::get_https_client;
use crate::{messages::cert::SetAcmeDnsRecord, nats::TypedNats};
use acme2::{
    gen_rsa_private_key, AccountBuilder, AuthorizationStatus, ChallengeStatus, Csr,
    DirectoryBuilder, OrderBuilder, OrderStatus,
};
use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use openssl::{
    asn1::Asn1Time,
    pkey::{PKey, Private},
    x509::X509,
};
use reqwest::Client;
use std::{path::Path, time::Duration};

use super::cli::CertOptions;

mod https_client;

const DNS_01: &str = "dns-01";
const REFRESH_MARGIN: Duration = Duration::from_secs(3600 * 24 * 15);
const MAX_SLEEP: Duration = Duration::from_secs(3600);

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
        nats.request(
            &SetAcmeDnsRecord::subject(),
            &SetAcmeDnsRecord {
                cluster: cluster_domain.to_string(),
                value,
            },
        )
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

pub async fn refresh_certificate(cert_options: &CertOptions) -> Result<()> {
    let client = get_https_client()?;
    let nats = cert_options.nats.connection().await?;

    let (pkey, cert) = get_certificate(
        &cert_options.cluster_domain,
        &nats,
        &cert_options.acme_server_url,
        &client,
    )
    .await?;

    std::fs::write(&cert_options.key_paths.certificate_path, cert.to_pem()?)?;
    std::fs::write(
        &cert_options.key_paths.private_key_path,
        pkey.private_key_to_pem_pkcs8()?,
    )?;

    Ok(())
}

pub fn cert_validity(certificate_path: &Path) -> Option<DateTime<Utc>> {
    let cert_pem = std::fs::read(certificate_path).ok()?;
    let cert = X509::from_pem(&cert_pem).ok()?;
    let not_after_asn1 = cert.not_after();
    let not_after_unix = Asn1Time::from_unix(0).ok()?.diff(not_after_asn1).ok()?;
    let not_after_naive = NaiveDateTime::from_timestamp(
        not_after_unix.days as i64 * 86400 + not_after_unix.secs as i64,
        0,
    );
    Some(DateTime::from_utc(not_after_naive, Utc))
}

pub async fn refresh_if_not_valid(cert_options: &CertOptions) -> Result<Option<Duration>> {
    if let Some(valid_until) = cert_validity(&cert_options.key_paths.certificate_path) {
        let refresh_at = valid_until.checked_sub_signed(chrono::Duration::from_std(REFRESH_MARGIN)?).ok_or_else(|| anyhow!("Date subtraction would result in over/underflow, this should never happen."))?;
        let time_until_refresh = refresh_at.signed_duration_since(Utc::now());

        if time_until_refresh > chrono::Duration::zero() {
            return Ok(Some(time_until_refresh.to_std()?))
        }

        tracing::info!(
            ?valid_until,
            "Certificate exists, but is ready for refresh."
        );
    }

    tracing::info!("Refreshing certificate.");
    refresh_certificate(&cert_options).await?;
    tracing::info!("Done refreshing certificate.");

    Ok(None)
}

pub async fn refresh_loop(cert_options: CertOptions) -> Result<()> {
    loop {
        match refresh_if_not_valid(&cert_options).await {
            Ok(Some(valid_until)) => tokio::time::sleep(valid_until.min(MAX_SLEEP)).await,
            Ok(None) => tokio::time::sleep(MAX_SLEEP).await,
            Err(error) => {
                tracing::warn!(?error, "Error issuing certificate, will try again.");
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
    }
}
