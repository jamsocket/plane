use crate::keys::{load_certs, load_private_key, KeyCertPaths};
use anyhow::{Context, Result};
use notify::{
    event::{AccessKind, AccessMode},
    recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use rustls::{
    server::ResolvesServerCert,
    sign::{any_supported_type, CertifiedKey},
    Certificate, PrivateKey,
};
use std::sync::Arc;
use tokio::sync::watch::{channel, Receiver, Sender};

pub struct CertRefresher {
    receiver: Receiver<Option<Arc<CertifiedKey>>>,
    _watcher: RecommendedWatcher,
}

impl CertRefresher {
    pub fn new(key_cert_path_pair: KeyCertPaths) -> Result<Self> {
        let (sender, receiver) = channel(None);

        if let Err(error) = Self::try_to_update_certs(&key_cert_path_pair, &sender) {
            tracing::warn!(?error, "Certificates aren't ready yet. This is expected if the certificate fetcher has just started.")
        }

        let mut watcher = {
            let key_cert_path_pair = key_cert_path_pair.clone();
            let mut private_key: Option<PrivateKey> = None;
            let mut certificate: Option<Vec<Certificate>> = None;

            recommended_watcher(move |ev| {
                if let Ok(Event {
                    kind: EventKind::Access(AccessKind::Close(AccessMode::Write)),
                    paths,
                    ..
                }) = ev
                {
                    if paths.contains(&key_cert_path_pair.key_path) {
                        match load_private_key(&key_cert_path_pair.key_path) {
                            Ok(key) => private_key = Some(key),
                            Err(err) => {
                                tracing::warn!(?err, "Error loading private key.");
                                return;
                            }
                        }
                    }

                    if paths.contains(&key_cert_path_pair.cert_path) {
                        match load_certs(&key_cert_path_pair.cert_path) {
                            Ok(cert) => certificate = Some(cert),
                            Err(err) => {
                                tracing::warn!(?err, "Error loading certificate.");
                                return;
                            }
                        }
                    }

                    if let (Some(key), Some(cert)) = (&private_key, &certificate) {
                        tracing::info!("Updating key/cert pair.");

                        let private_key = match any_supported_type(key) {
                            Err(err) => {
                                tracing::warn!(?err, "Key error.");
                                return;
                            }
                            Ok(pk) => pk,
                        };
                        let ck = CertifiedKey::new(cert.clone(), private_key);
                        let _ = sender.send(Some(Arc::new(ck)));
                    }
                }
            })?
        };

        for path in key_cert_path_pair.parent_paths() {
            watcher
                .watch(&path, RecursiveMode::NonRecursive)
                .context("Error installing watcher.")?;
        }

        let resolver = CertRefresher {
            receiver,
            _watcher: watcher,
        };

        Ok(resolver)
    }

    fn try_to_update_certs(
        key_cert_path_pair: &KeyCertPaths,
        sender: &Sender<Option<Arc<CertifiedKey>>>,
    ) -> Result<()> {
        let cert_key_pair = Some(Arc::new(
            key_cert_path_pair
                .load_certified_key()
                .context("Error loading cert pair.")?,
        ));
        let _ = sender.send(cert_key_pair);

        Ok(())
    }

    pub fn resolver(&self) -> CertResolver {
        CertResolver {
            receiver: self.receiver.clone(),
        }
    }
}

pub struct CertResolver {
    receiver: Receiver<Option<Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for CertResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.receiver.borrow().clone()
    }
}
