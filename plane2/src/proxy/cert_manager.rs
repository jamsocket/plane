use super::{cert_pair::CertificatePair, AcmeConfig};
use crate::{
    protocol::{CertManagerRequest, CertManagerResponse},
    types::ClusterId,
};
use acme2_eab::{
    gen_rsa_private_key, AccountBuilder, AuthorizationStatus, ChallengeStatus, Csr,
    DirectoryBuilder, OrderBuilder, OrderStatus,
};
use anyhow::{anyhow, Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};
use tokio::sync::{
    broadcast,
    watch::{Receiver, Sender},
};
use tokio_rustls::rustls::{
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};

const DNS_01: &str = "dns-01";
const MAX_SLEEP_TIME: Duration = Duration::from_secs(30 * 60); // 30 minutes
const LOCK_SLEEP_TIME: Duration = Duration::from_secs(60); // 1 minute
const RENEWAL_WINDOW: Duration = Duration::from_secs(24 * 60 * 60 * 30); // 30 days

/// Handle for receiving new certificates. Implements ResolvesServerCert, which
/// allows it to be used as a rustls cert_resolver.
pub struct CertWatcher {
    receiver: Receiver<Option<Arc<CertifiedKey>>>,
}

impl CertWatcher {
    fn new(receiver: Receiver<Option<Arc<CertifiedKey>>>) -> Self {
        Self { receiver }
    }

    pub async fn next(&mut self) -> Option<Arc<CertifiedKey>> {
        tracing::info!("Waiting for next certificate...");
        self.receiver
            .changed()
            .await
            .expect("Certificate receiver should not be closed.");
        tracing::info!("Received new certificate.");
        self.receiver.borrow().clone()
    }
}

impl ResolvesServerCert for CertWatcher {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.receiver.borrow().clone()
    }
}

pub struct CertManager {
    cluster: ClusterId,
    send_cert: Arc<Sender<Option<Arc<CertifiedKey>>>>,
    refresh_loop: Option<tokio::task::JoinHandle<()>>,
    current_cert: Arc<RwLock<Option<CertificatePair>>>,
    response_sender: broadcast::Sender<CertManagerResponse>,
    acme_config: Option<AcmeConfig>,
    path: Option<PathBuf>,
}

impl CertManager {
    pub fn new(
        cluster: ClusterId,
        send_cert: Sender<Option<Arc<CertifiedKey>>>,
        cert_path: Option<&Path>,
        acme_config: Option<AcmeConfig>,
    ) -> Result<Self> {
        let initial_cert = if let Some(cert_path) = cert_path {
            if cert_path.exists() {
                Some(CertificatePair::load(cert_path)?)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(cert) = &initial_cert {
            tracing::info!(
                "Loaded certificate for {} (valid from {:?} to {:?})",
                cert.common_name,
                cert.validity_start,
                cert.validity_end
            );

            send_cert.send(Some(Arc::new(cert.certified_key.clone())))?;
        }

        let (response_sender, _) = broadcast::channel(1);

        Ok(Self {
            cluster,
            send_cert: Arc::new(send_cert),
            refresh_loop: None,
            acme_config,
            current_cert: Arc::new(RwLock::new(initial_cert)),
            path: cert_path.map(|p| p.to_owned()),
            response_sender,
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
            let current_cert = self.current_cert.clone();
            let response_sender = self.response_sender.subscribe();
            let path = self.path.clone();

            let handle = tokio::spawn(refresh_loop(
                self.cluster.clone(),
                acme_config.clone(),
                send_cert,
                sender,
                current_cert,
                response_sender,
                path,
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

pub fn watcher_manager_pair(
    cluster: ClusterId,
    path: Option<&Path>,
    acme_config: Option<AcmeConfig>,
) -> Result<(CertWatcher, CertManager)> {
    let (send_cert, recv_cert) = tokio::sync::watch::channel::<Option<Arc<CertifiedKey>>>(None);

    let cert_watcher = CertWatcher::new(recv_cert);
    let cert_manager = CertManager::new(cluster, send_cert, path, acme_config)?;

    Ok((cert_watcher, cert_manager))
}

pub async fn do_refresh(
    cluster: &ClusterId,
    acme_config: &AcmeConfig,
    send_cert: &Arc<Sender<Option<Arc<CertifiedKey>>>>,
    request_sender: &(impl Fn(CertManagerRequest) + Send + Sync + 'static),
    current_cert: &Arc<RwLock<Option<CertificatePair>>>,
    response_receiver: &mut broadcast::Receiver<CertManagerResponse>,
    path: Option<&PathBuf>,
) -> Result<()> {
    let last_current_cert = current_cert.read().expect("Cert lock is poisoned.").clone();

    if let Some(current_cert) = last_current_cert {
        let renewal_time = current_cert
            .validity_end
            .checked_sub(RENEWAL_WINDOW)
            .expect("Cert validity date arithmetic failed.");
        let time_until_renew = renewal_time.duration_since(SystemTime::now())?;

        if time_until_renew > Duration::ZERO {
            tracing::info!(
                "Certificate for {} is valid until {:?}. Renewal scheduled for {:?} ({:?} from now). Sleeping.",
                current_cert.common_name,
                current_cert.validity_end,
                renewal_time,
                time_until_renew
            );
            tokio::time::sleep(time_until_renew.max(MAX_SLEEP_TIME)).await;
            return Ok(());
        }
    }

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

    let result = get_certificate(cluster, acme_config, request_sender, response_receiver).await;

    match result {
        Ok(cert_pair) => {
            tracing::info!("Got certificate.");
            let cert = cert_pair.certified_key.clone();
            send_cert.send(Some(Arc::new(cert)))?;
            let mut current_cert = current_cert.write().expect("Cert lock is poisoned.");
            if let Some(path) = path.as_ref() {
                cert_pair.save(path)?;
            }
            *current_cert = Some(cert_pair);
        }
        Err(err) => {
            tracing::error!(?err, "Error getting certificate.");
            tokio::time::sleep(LOCK_SLEEP_TIME).await;
        }
    }

    Ok(())
}

pub async fn refresh_loop(
    cluster: ClusterId,
    acme_config: AcmeConfig,
    send_cert: Arc<Sender<Option<Arc<CertifiedKey>>>>,
    request_sender: impl Fn(CertManagerRequest) + Send + Sync + 'static,
    current_cert: Arc<RwLock<Option<CertificatePair>>>,
    mut response_receiver: broadcast::Receiver<CertManagerResponse>,
    path: Option<PathBuf>,
) {
    loop {
        let result = do_refresh(
            &cluster,
            &acme_config,
            &send_cert,
            &request_sender,
            &current_cert,
            &mut response_receiver,
            path.as_ref(),
        )
        .await;

        if let Err(err) = result {
            tracing::error!(?err, "Error refreshing certificate.");
        }
    }
}

pub async fn get_certificate(
    cluster: &ClusterId,
    acme_config: &AcmeConfig,
    request_sender: &(impl Fn(CertManagerRequest) + Send + Sync + 'static),
    response_receiver: &mut broadcast::Receiver<CertManagerResponse>,
) -> anyhow::Result<CertificatePair> {
    let dir = DirectoryBuilder::new(acme_config.endpoint.to_string())
        .http_client(acme_config.client.clone())
        .build()
        .await
        .context("Building directory")?;

    let mut builder = AccountBuilder::new(dir);
    // builder.private_key(...);
    builder.contact(vec![format!("mailto:{}", acme_config.mailto_email)]);
    // if let Some(acme_eab_keypair) = acme_eab_keypair {
    //     let eab_key = PKey::hmac(&acme_eab_keypair.key)?;
    //     builder.external_account_binding(acme_eab_keypair.key_id.clone(), eab_key);
    // }
    builder.terms_of_service_agreed(true);
    let account = builder.build().await.context("Building account")?;

    let mut builder = OrderBuilder::new(account);
    builder.add_dns_identifier(format!("*.{}", cluster));
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

        tracing::info!(?txt_value, "Requesting TXT record from platform.");

        request_sender(CertManagerRequest::SetTxtRecord { txt_value });
        tracing::info!("Waiting for response from cert manager.");

        let response = match response_receiver.recv().await {
            Ok(response) => response,
            Err(err) => {
                tracing::error!(?err, "Cert manager error.");
                return Err(anyhow!("Cert manager error."));
            }
        };
        tracing::info!(?response, "Received response from cert manager.");

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
