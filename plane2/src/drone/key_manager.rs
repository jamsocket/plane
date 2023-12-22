use super::executor::Executor;
use crate::{
    names::BackendName,
    protocol::{AcquiredKey, BackendAction, RenewKeyRequest},
    typed_socket::TypedSocketSender,
    types::TerminationKind,
    util::GuardHandle,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::sleep;

pub struct KeyManager {
    executor: Arc<Executor>,

    /// Map from a backend to the most recently acquired key for that backend,
    /// along with the handle for the task that is responsible for renewing the key
    /// and terminating the backend if the key cannot be renewed.
    handles: HashMap<BackendName, (AcquiredKey, GuardHandle)>,

    sender: Option<TypedSocketSender<RenewKeyRequest>>,
}

async fn renew_key_loop(
    key: AcquiredKey,
    sender: Option<TypedSocketSender<RenewKeyRequest>>,
    executor: Arc<Executor>,
) {
    loop {
        let now = SystemTime::now();
        if now >= key.hard_terminate_at {
            tracing::warn!("Key {:?} has expired, hard-terminating.", key.key);
            if let Err(err) = executor
                .apply_action(
                    &key.backend,
                    &BackendAction::Terminate {
                        kind: TerminationKind::Hard,
                    },
                )
                .await
            {
                tracing::error!(%err, "Error hard-terminating backend.");
                sleep(Duration::from_secs(1)).await;
                continue;
            }
            break;
        }

        if now >= key.soft_terminate_at {
            tracing::warn!("Key {:?} has expired, soft-terminating.", key.key);
            if let Err(err) = executor
                .apply_action(
                    &key.backend,
                    &BackendAction::Terminate {
                        kind: TerminationKind::Soft,
                    },
                )
                .await
            {
                tracing::error!(%err, "Error soft-terminating backend.");
                sleep(Duration::from_secs(1)).await;
                continue;
            }

            if let Ok(time_to_sleep) = key.hard_terminate_at.duration_since(now) {
                sleep(time_to_sleep).await;
            }

            continue;
        }

        if now >= key.renew_at {
            tracing::info!("Renewing key {:?}.", key.key);

            if let Some(ref sender) = sender {
                let request = RenewKeyRequest {
                    key: key.key.clone(),
                    token: key.token,
                    backend: key.backend.clone(),
                    local_time: SystemTime::now(),
                };

                if let Err(err) = sender.send(request) {
                    tracing::error!(%err, "Error sending renew key request.");
                }
            }

            if let Ok(time_to_sleep) = key.soft_terminate_at.duration_since(now) {
                sleep(time_to_sleep).await;
            }
            continue;
        }

        if let Ok(time_to_sleep) = key.renew_at.duration_since(now) {
            sleep(time_to_sleep).await;
        }
    }
}

impl KeyManager {
    pub fn new(executor: Arc<Executor>) -> Self {
        Self {
            executor,
            handles: HashMap::new(),
            sender: None,
        }
    }

    pub fn set_sender(&mut self, sender: TypedSocketSender<RenewKeyRequest>) {
        self.sender.replace(sender);

        for (_, (acquired_key, handle)) in self.handles.iter_mut() {
            let mut new_handle = GuardHandle::new(renew_key_loop(
                acquired_key.clone(),
                self.sender.clone(),
                self.executor.clone(),
            ));

            std::mem::swap(handle, &mut new_handle);
        }
    }

    pub fn register_key(&mut self, key: AcquiredKey) {
        let handle = GuardHandle::new(renew_key_loop(
            key.clone(),
            self.sender.clone(),
            self.executor.clone(),
        ));

        self.handles.insert(key.backend.clone(), (key, handle));
    }

    pub fn unregister_key(&mut self, backend: &BackendName) {
        self.handles.remove(backend);
    }
}
