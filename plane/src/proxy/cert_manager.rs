use super::{cert_pair::CertificatePair, AcmeConfig};
use crate::{
    log_types::LoggableTime,
    protocol::{CertManagerRequest, CertManagerResponse},
    types::ClusterName,
};
use acme2_eab::{
    gen_rsa_private_key, Account, AccountBuilder, AuthorizationStatus, ChallengeStatus, Csr,
    DirectoryBuilder, OrderBuilder, OrderStatus,
};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use dynamic_proxy::tokio_rustls::rustls::{
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use std::{
    ops::Sub,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{
    broadcast,
    watch::{Receiver, Sender},
};
use valuable::Valuable;

const DNS_01: &str = "dns-01";

/// How long to sleep after failing to acquire the certificate lease.
const LOCK_SLEEP_TIME: Duration = Duration::from_secs(60); // 1 minute

/// How long to sleep after failing to renew the certificate.
const FAILURE_SLEEP_TIME: Duration = Duration::from_secs(60 * 5); // 5 minutes

/// How long in advance of the certificate expiring to renew it.
const RENEWAL_WINDOW: Duration = Duration::from_secs(24 * 60 * 60 * 30); // 30 days

/// Handle for receiving new certificates. Implements ResolvesServerCert, which
/// allows it to be used as a rustls cert_resolver.
pub struct CertWatcher {
    receiver: Receiver<Option<CertificatePair>>,
    certified_key: Mutex<Option<Arc<CertifiedKey>>>,
}

impl CertWatcher {
    fn new(receiver: Receiver<Option<CertificatePair>>) -> Self {
        Self {
            receiver,
            certified_key: Mutex::new(None),
        }
    }

    /// Update the certified key to match the latest CertificatePair from the receiver.
    fn update_certified_key(&self) {
        let cert_pair = self.receiver.borrow().as_ref().cloned();
        let mut lock = self
            .certified_key
            .lock()
            .expect("Certified key lock poisoned.");

        if let Some(cert_pair) = cert_pair.as_ref() {
            lock.replace(Arc::new(cert_pair.certified_key.clone()));
        } else {
            lock.take();
        }
    }

    pub async fn wait_for_initial_cert(&mut self) -> Result<()> {
        loop {
            if self
                .certified_key
                .lock()
                .expect("Certified key lock poisoned")
                .is_some()
            {
                return Ok(());
            }

            self.receiver
                .changed()
                .await
                .expect("Failed to receive from channel.");
            self.update_certified_key();
        }
    }
}

impl std::fmt::Debug for CertWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CertWatcher")
    }
}

impl ResolvesServerCert for CertWatcher {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if self
            .receiver
            .has_changed()
            .expect("Receiver channel should not be closed.")
        {
            self.update_certified_key();
        }

        self.certified_key
            .lock()
            .expect("Certified key lock poisoned.")
            .clone()
    }
}

/// Manages the certificate refresh loop.
pub struct CertManager {
    cluster: ClusterName,

    /// Channel for sending new certificates to the CertWatcher.
    send_cert: Arc<Sender<Option<CertificatePair>>>,
    refresh_loop: Option<tokio::task::JoinHandle<()>>,

    /// Configuration used for the ACME certificate request.
    acme_config: Option<AcmeConfig>,

    /// Sender for forwarding responses from the controller to the refresh loop
    /// task.
    response_sender: broadcast::Sender<CertManagerResponse>,

    /// Path to save the certificate to.
    path: Option<PathBuf>,
}

impl CertManager {
    pub async fn new(
        cluster: ClusterName,
        send_cert: Sender<Option<CertificatePair>>,
        cert_path: Option<&Path>,
        acme_config: Option<AcmeConfig>,
    ) -> Result<Self> {
        let initial_cert = if let Some(cert_path) = cert_path {
            if cert_path.exists() {
                match CertificatePair::load(cert_path) {
                    Ok(cert) => Some(cert),
                    Err(err) => {
                        tracing::error!(
                            ?err,
                            ?cert_path,
                            "Error loading certificate; obtaining via ACME instead."
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(cert) = initial_cert {
            tracing::info!(
                "Loaded certificate for {} (valid from {:?} to {:?})",
                cert.common_name,
                cert.validity_start,
                cert.validity_end
            );

            send_cert.send(Some(cert))?;
        }

        let (response_sender, _) = broadcast::channel(1);

        Ok(Self {
            cluster,
            send_cert: Arc::new(send_cert),
            refresh_loop: None,
            path: cert_path.map(|p| p.to_owned()),
            response_sender,
            acme_config,
        })
    }

    pub fn set_request_sender<F>(&mut self, sender: F)
    where
        F: Fn(CertManagerRequest) + Send + Sync + 'static,
    {
        if let Some(handle) = self.refresh_loop.take() {
            handle.abort();
        }

        if let Some(acme_config) = self.acme_config.as_ref() {
            let send_cert = self.send_cert.clone();
            let response_sender = self.response_sender.subscribe();
            let path = self.path.clone();

            let handle = tokio::spawn(refresh_loop(
                self.cluster.clone(),
                send_cert,
                sender,
                response_sender,
                path,
                acme_config.clone(),
            ));

            self.refresh_loop = Some(handle);
        }
    }

    pub fn receive(&self, response: CertManagerResponse) {
        self.response_sender
            .send(response)
            .expect("Cert manager response receiver closed.");
    }
}

/// Create a CertWatcher and CertManager pair.
pub async fn watcher_manager_pair(
    cluster: ClusterName,
    path: Option<&Path>,
    acme_config: Option<AcmeConfig>,
) -> Result<(CertWatcher, CertManager)> {
    let (send_cert, recv_cert) = tokio::sync::watch::channel::<Option<CertificatePair>>(None);

    let cert_watcher = CertWatcher::new(recv_cert);
    let cert_manager = CertManager::new(cluster, send_cert, path, acme_config).await?;

    Ok((cert_watcher, cert_manager))
}

/// One step of the refresh loop.
/// If we have a valid certificate, sleep until it's time to renew it.
/// Otherwise, request a certificate lease from the cert manager, and then
/// request a certificate from the ACME server.
async fn refresh_loop_step(
    maybe_account: &mut Option<Arc<Account>>,
    acme_config: &AcmeConfig,
    cluster: &ClusterName,
    send_cert: &Arc<Sender<Option<CertificatePair>>>,
    request_sender: &(impl Fn(CertManagerRequest) + Send + Sync + 'static),
    response_receiver: &mut broadcast::Receiver<CertManagerResponse>,
    path: Option<&PathBuf>,
) -> Result<()> {
    let last_current_cert = send_cert.borrow().clone();

    if let Some(current_cert) = last_current_cert {
        let renewal_time = LoggableTime(current_cert.validity_end.0.sub(RENEWAL_WINDOW));
        let time_until_renew = renewal_time.0.sub(Utc::now());

        if time_until_renew > chrono::Duration::zero() {
            tracing::info!(
                common_name = current_cert.common_name,
                validity_end = current_cert.validity_end.as_value(),
                renewal_time = renewal_time.as_value(),
                days_until_renew = time_until_renew.num_days(),
                "Obtained certificate.",
            );
            tokio::time::sleep(
                time_until_renew
                    .to_std()
                    .expect("time_until_renew is always positive."),
            )
            .await;
            return Ok(());
        }
    }

    // We only need to create one account per drone, so we store it in `maybe_account`,
    // a mutable `Option` reference which is used across calls to `refresh_loop_step`.
    let account = match maybe_account {
        Some(account) => account,
        None => {
            let client = if acme_config.accept_insecure_certs_for_testing {
                tracing::warn!("ACME server certificate chain will not be validated! This is ONLY for testing, and should not be used otherwise.");
                reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .build()?
            } else {
                reqwest::Client::new()
            };

            let dir = DirectoryBuilder::new(acme_config.endpoint.to_string())
                .http_client(client)
                .build()
                .await
                .context("Building directory")?;

            let mut builder = AccountBuilder::new(dir);
            builder.contact(vec![format!("mailto:{}", acme_config.mailto_email)]);
            if let Some(acme_eab_keypair) = acme_config.acme_eab_keypair.clone() {
                let eab_key = openssl::pkey::PKey::hmac(&acme_eab_keypair.key_bytes()?)?;
                builder.external_account_binding(acme_eab_keypair.key_id.clone(), eab_key);
            }
            builder.terms_of_service_agreed(true);
            let account = builder.build().await.context("Building account")?;

            maybe_account.insert(account)
        }
    };

    tracing::info!("Requesting certificate lease.");
    request_sender(CertManagerRequest::CertLeaseRequest);

    // wait for response.
    let response = match response_receiver.recv().await {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(?err, "Cert manager error.");
            return Ok(());
        }
    };

    match response {
        CertManagerResponse::CertLeaseResponse { accepted: true } => (),
        CertManagerResponse::CertLeaseResponse { accepted: false } => {
            tracing::warn!("Cert manager rejected cert lease request. Sleeping.");
            tokio::time::sleep(LOCK_SLEEP_TIME).await;
            return Ok(());
        }
        _ => {
            tracing::error!("Unexpected response from cert manager.");
            return Ok(());
        }
    }

    tracing::info!("Cert manager accepted cert lease request.");

    let result = get_certificate(account.clone(), cluster, request_sender, response_receiver).await;

    match result {
        Ok(cert_pair) => {
            tracing::info!("Got certificate.");
            send_cert.send(Some(cert_pair.clone()))?;
            if let Some(path) = path.as_ref() {
                cert_pair.save(path)?;
            }
        }
        Err(err) => {
            tracing::error!(?err, "Error getting certificate.");
            tokio::time::sleep(FAILURE_SLEEP_TIME).await;
        }
    }

    Ok(())
}

pub async fn refresh_loop(
    cluster: ClusterName,
    send_cert: Arc<Sender<Option<CertificatePair>>>,
    request_sender: impl Fn(CertManagerRequest) + Send + Sync + 'static,
    mut response_receiver: broadcast::Receiver<CertManagerResponse>,
    path: Option<PathBuf>,
    acme_config: AcmeConfig,
) {
    let mut account: Option<Arc<Account>> = None;

    loop {
        let result = refresh_loop_step(
            &mut account,
            &acme_config,
            &cluster,
            &send_cert,
            &request_sender,
            &mut response_receiver,
            path.as_ref(),
        )
        .await;

        if let Err(err) = result {
            tracing::error!(?err, "Error refreshing certificate.");
        }
    }
}

async fn get_certificate(
    account: Arc<Account>,
    cluster: &ClusterName,
    request_sender: &(impl Fn(CertManagerRequest) + Send + Sync + 'static),
    response_receiver: &mut broadcast::Receiver<CertManagerResponse>,
) -> anyhow::Result<CertificatePair> {
    let mut builder = OrderBuilder::new(account);
    builder.add_dns_identifier(format!("{}", cluster));
    builder.add_dns_identifier(format!("*.{}", cluster)); // wildcard
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

        tracing::info!(?challenge, "Received challenge.");

        let txt_value = challenge
            .key_authorization_encoded()
            .context("Encoding authorization")?
            .context("No authorization value.")?;

        tracing::info!(txt_value, "Requesting TXT record from platform.");

        request_sender(CertManagerRequest::SetTxtRecord { txt_value });
        tracing::info!("Waiting for response from cert manager.");

        let response = match response_receiver.recv().await {
            Ok(response) => response,
            Err(err) => {
                tracing::error!(?err, "Cert manager error.");
                return Err(anyhow!("Cert manager error."));
            }
        };
        tracing::info!(
            response = response.as_value(),
            "Received response from cert manager."
        );

        match response {
            CertManagerResponse::SetTxtRecordResponse { accepted: true } => (),
            CertManagerResponse::SetTxtRecordResponse { accepted: false } => {
                tracing::warn!("Cert manager rejected TXT record request.");
                return Err(anyhow!("Cert manager rejected TXT record request."));
            }
            _ => {
                tracing::error!("Unexpected response from cert manager.");
                return Err(anyhow!("Unexpected response from cert manager."));
            }
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

    let pkey_der = pkey.private_key_to_der()?;
    let cert_ders = cert
        .into_iter()
        .map(|cert| cert.to_der())
        .collect::<Result<Vec<_>, _>>()?;

    let cert_pair = CertificatePair::from_raw_ders(&pkey_der, &cert_ders)?;

    Ok(cert_pair)
}
