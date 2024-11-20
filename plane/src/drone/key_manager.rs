use super::executor::Executor;
use crate::{
    log_types::LoggableTime,
    names::BackendName,
    protocol::{AcquiredKey, BackendAction, KeyDeadlines, RenewKeyRequest},
    typed_socket::TypedSocketSender,
    types::{backend_state::TerminationReason, TerminationKind},
    util::GuardHandle,
};
use chrono::Utc;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::sleep;
use valuable::Valuable;

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
    backend: BackendName,
    sender: Option<TypedSocketSender<RenewKeyRequest>>,
    executor: Arc<Executor>,
) {
    loop {
        let now = Utc::now();
        let deadlines = &key.deadlines;
        if now >= deadlines.hard_terminate_at.0 {
            tracing::warn!("Key {:?} has expired, hard-terminating.", key.key);
            if let Err(err) = executor
                .apply_action(
                    &backend,
                    &BackendAction::Terminate {
                        kind: TerminationKind::Hard,
                        reason: TerminationReason::KeyExpired,
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

        if now >= deadlines.soft_terminate_at.0 {
            tracing::warn!("Key {:?} has expired, soft-terminating.", key.key);
            if let Err(err) = executor
                .apply_action(
                    &backend,
                    &BackendAction::Terminate {
                        kind: TerminationKind::Soft,
                        reason: TerminationReason::KeyExpired,
                    },
                )
                .await
            {
                tracing::error!(%err, "Error soft-terminating backend.");
                sleep(Duration::from_secs(1)).await;
                continue;
            }

            if let Ok(time_to_sleep) = deadlines
                .hard_terminate_at
                .0
                .signed_duration_since(now)
                .to_std()
            {
                sleep(time_to_sleep).await;
            }

            continue;
        }

        if now >= deadlines.renew_at.0 {
            tracing::info!(key = key.key.as_value(), "Renewing key.");

            if let Some(ref sender) = sender {
                let request = RenewKeyRequest {
                    backend: backend.clone(),
                    local_time: LoggableTime(Utc::now()),
                };

                if let Err(err) = sender.send(request) {
                    tracing::error!(%err, "Error sending renew key request.");
                }
            }

            if let Ok(time_to_sleep) = deadlines
                .soft_terminate_at
                .0
                .signed_duration_since(now)
                .to_std()
            {
                sleep(time_to_sleep).await;
            }
            continue;
        }

        if let Ok(time_to_sleep) = deadlines.renew_at.0.signed_duration_since(now).to_std() {
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

        for (backend, (acquired_key, handle)) in self.handles.iter_mut() {
            let new_handle = GuardHandle::new(renew_key_loop(
                acquired_key.clone(),
                backend.clone(),
                self.sender.clone(),
                self.executor.clone(),
            ));

            *handle = new_handle;
        }
    }

    /// Register a key with the key manager, ensuring that it will be renewed.
    /// Returns true if the key was registered, and false if it was already registered (this
    /// should be considered an error.)
    pub fn register_key(&mut self, backend: BackendName, key: AcquiredKey) -> bool {
        if self.handles.contains_key(&backend) {
            return false;
        }

        let handle = GuardHandle::new(renew_key_loop(
            key.clone(),
            backend.clone(),
            self.sender.clone(),
            self.executor.clone(),
        ));

        self.handles.insert(backend, (key, handle));

        true
    }

    pub fn update_deadlines(&mut self, backend: &BackendName, deadlines: KeyDeadlines) {
        if let Some((key, handle)) = self.handles.get_mut(backend) {
            key.deadlines = deadlines;

            *handle = GuardHandle::new(renew_key_loop(
                key.clone(),
                backend.clone(),
                self.sender.clone(),
                self.executor.clone(),
            ));
        }
    }

    pub fn unregister_key(&mut self, backend: &BackendName) {
        self.handles.remove(backend);
    }
}
