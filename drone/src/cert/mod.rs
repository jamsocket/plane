use crate::keys::KeyCertPaths;
use acme::AcmeEabConfiguration;
use acme2_eab::{
    gen_rsa_private_key, AccountBuilder, AuthorizationStatus, ChallengeStatus, Csr,
    DirectoryBuilder, OrderBuilder, OrderStatus,
};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use openssl::{
    asn1::Asn1Time,
    pkey::{PKey, Private},
    x509::X509,
};
use plane_core::{
    messages::cert::SetAcmeDnsRecord, nats::TypedNats, types::ClusterName, NeverResult,
};
use reqwest::Client;
use std::io::Write;
use std::{fs::File, path::Path, time::Duration};

pub mod acme;

const DNS_01: &str = "dns-01";
const REFRESH_MARGIN: Duration = Duration::from_secs(3600 * 24 * 15);
const MAX_SLEEP: Duration = Duration::from_secs(3600);

#[derive(Clone)]
pub struct CertOptions {
    pub cluster_domain: String,
    pub nats: TypedNats,
    pub key_paths: KeyCertPaths,
    pub email: String,
    pub acme_server_url: String,
    pub acme_eab_keypair: Option<AcmeEabConfiguration>,
}

pub async fn get_certificate(
    cluster_domain: &str,
    nats: &TypedNats,
    acme_server_url: &str,
    mailto_email: &str,
    client: &Client,
    acme_eab_keypair: Option<&AcmeEabConfiguration>,
    account_pkey: &PKey<Private>,
) -> Result<(PKey<Private>, Vec<X509>)> {
    let _span = tracing::info_span!("Getting certificate", %cluster_domain);
    let _span_guard = _span.enter();

    let dir = DirectoryBuilder::new(acme_server_url.to_string())
        .http_client(client.clone())
        .build()
        .await
        .context("Building directory")?;

    let mut builder = AccountBuilder::new(dir);
    builder.private_key(account_pkey.clone());
    builder.contact(vec![format!("mailto:{}", mailto_email)]);
    if let Some(acme_eab_keypair) = acme_eab_keypair {
        let eab_key = PKey::hmac(&acme_eab_keypair.key).unwrap();
        builder.external_account_binding(acme_eab_keypair.key_id.clone(), eab_key);
    }

    builder.terms_of_service_agreed(true);
    let account = builder.build().await.context("Building account")?;

    let mut builder = OrderBuilder::new(account);
    builder.add_dns_identifier(format!("*.{}", cluster_domain));
    let order = builder.build().await.context("Building order")?;

    let authorizations = order
        .authorizations()
        .await
        .context("Fetching authorizations")?;
    for auth in authorizations {
        tracing::info!("Requesting challenge.");
        let challenge = auth
            .get_challenge(DNS_01)
            .context("Obtaining dns-01 challenge")?;

        let value = challenge
            .key_authorization_encoded()
            .context("Encoding authorization")?
            .context("No authorization value.")?;

        tracing::info!(?value, "Requesting TXT record from platform.");

        let result = nats
            .request(&SetAcmeDnsRecord {
                cluster: ClusterName::new(cluster_domain),
                value: value.clone(),
            })
            .await
            .context("Setting ACME DNS record")?;

        if !result {
            return Err(anyhow!("Platform rejected TXT record."));
        }

        if challenge.status != ChallengeStatus::Valid {
            tracing::info!("Validating challenge.");
            let challenge = challenge.validate().await.context("Validating challenge")?;
            let challenge = challenge
                .wait_done(Duration::from_secs(5), 3)
                .await
                .context("Waiting for challenge")?;
            if challenge.status != ChallengeStatus::Valid {
                tracing::warn!(?challenge, "Challenge status is not valid.");
                return Err(anyhow!("ACME challenge failed."));
            }
        } else {
            tracing::info!("Challenge already valid.");
        }

        tracing::info!("Validating authorization.");
        let authorization = auth
            .wait_done(Duration::from_secs(5), 3)
            .await
            .context("Waiting for authorization")?;
        if authorization.status != AuthorizationStatus::Valid {
            tracing::warn!(?authorization, "Authorization status not valid.");
            return Err(anyhow!("ACME authorization failed."));
        }
    }

    tracing::info!("Waiting for order to become ready.");
    let order = order
        .wait_ready(Duration::from_secs(5), 3)
        .await
        .context("Waiting for order ready")?;
    if order.status != OrderStatus::Ready {
        tracing::warn!(?order, "Order status is not ready.");
        return Err(anyhow!("ACME order failed."));
    }

    tracing::info!("Waiting for order to become done.");
    let pkey = gen_rsa_private_key(4096)?;
    let order = order
        .finalize(Csr::Automatic(pkey.clone()))
        .await
        .context("Finalizing CSR")?;
    let order = order
        .wait_done(Duration::from_secs(5), 3)
        .await
        .context("Waiting for order to become done")?;

    if order.status != OrderStatus::Valid {
        tracing::warn!(?order, "ACME order not valid.");
        return Err(anyhow!("ACME order not valid."));
    }

    tracing::info!("Waiting for certificate.");
    let cert = order
        .certificate()
        .await
        .context("Getting certificate")?
        .context("ACME order response didn't include certificate.")?;

    if cert.is_empty() {
        tracing::warn!(?cert, "Certificate list is empty.");
        return Err(anyhow!("Certificate list is empty."));
    }

    tracing::info!("Got certificate from ACME.");

    Ok((pkey, cert))
}

pub async fn refresh_certificate(cert_options: &CertOptions, client: &Client) -> Result<()> {
    let nats = &cert_options.nats;
    let account_pkey = cert_options.key_paths.load_account_key()?;

    let (pkey, certs) = get_certificate(
        &cert_options.cluster_domain,
        nats,
        &cert_options.acme_server_url,
        &cert_options.email,
        client,
        cert_options.acme_eab_keypair.as_ref(),
        &account_pkey,
    )
    .await
    .context("Getting certificate")?;

    {
        let mut fh = File::options()
            .create(true)
            .write(true)
            .open(&cert_options.key_paths.cert_path)
            .context("Opening certificate file")?;

        for cert in certs {
            fh.write_all(&cert.to_pem().context("Converting cert to PEM")?)
                .context("Writing cert")?;
        }
    }

    std::fs::write(
        &cert_options.key_paths.key_path,
        pkey.private_key_to_pem_pkcs8()
            .context("Converting private key to pkcs8")?,
    )
    .context("Writing private key")?;

    Ok(())
}

pub fn cert_validity(certificate_path: &Path) -> Option<DateTime<Utc>> {
    let cert_pem = std::fs::read(certificate_path).ok()?;
    let cert = X509::from_pem(&cert_pem).ok()?;
    let not_after_asn1 = cert.not_after();
    let not_after_unix = Asn1Time::from_unix(0).ok()?.diff(not_after_asn1).ok()?;
    let not_after_naive = NaiveDateTime::from_timestamp_opt(
        i64::from(not_after_unix.days) * 86400 + i64::from(not_after_unix.secs),
        0,
    )
    .expect("from_timestamp_opt should not return a Some result on valid inputs.");
    Some(DateTime::from_utc(not_after_naive, Utc))
}

pub async fn refresh_if_not_valid(cert_options: &CertOptions) -> Result<Option<Duration>> {
    if let Some(valid_until) = cert_validity(&cert_options.key_paths.cert_path) {
        let refresh_at = valid_until
            .checked_sub_signed(chrono::Duration::from_std(REFRESH_MARGIN)?)
            .context(
                "Date subtraction would result in over/underflow, this should never happen.",
            )?;
        let time_until_refresh = refresh_at.signed_duration_since(Utc::now());

        if time_until_refresh > chrono::Duration::zero() {
            return Ok(Some(time_until_refresh.to_std()?));
        }

        tracing::info!(
            ?valid_until,
            "Certificate exists, but is ready for refresh."
        );
    }

    tracing::info!("Refreshing certificate.");
    refresh_certificate(cert_options, &Client::new())
        .await
        .context("Error refreshing certificate.")?;
    tracing::info!("Done refreshing certificate.");

    Ok(None)
}

pub async fn refresh_loop(cert_options: CertOptions) -> NeverResult {
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
