use super::state_store::StateStore;
use crate::{
    protocol::{AcquiredKey, RenewKeyRequest},
    typed_socket::TypedSocketSender,
    types::KeyConfig,
};
use std::{collections::HashMap, time::SystemTime};
use tokio::sync::watch::{Receiver, Sender};

pub struct KeyManager {
    state_store: StateStore,

    /// Map from a key to the thread that renews that key.
    // handles: HashMap<String, JoinHandle<()>>,
    senders: HashMap<KeyConfig, Sender<AcquiredKey>>,

    sender: TypedSocketSender<RenewKeyRequest>,
}

async fn renew_key_loop(
    key: AcquiredKey,
    sender: TypedSocketSender<RenewKeyRequest>,
    mut receiver: Receiver<AcquiredKey>,
) {
    loop {
        let Ok(()) = receiver.changed().await else {
            // Sender was dropped because KeyManager::unregister_key was called.
            break;
        };
        let key = receiver.borrow().clone();

        if let Ok(time_remaining_to_renew) = key.renew_at.duration_since(SystemTime::now()) {
            // renew_at is in the future, so we need to wait.
            tokio::time::sleep(time_remaining_to_renew).await;
        }

        let request = RenewKeyRequest {
            key: key.key.clone(),
            token: key.token.clone(),
            local_time: SystemTime::now(),
        };

        if let Err(err) = sender.send(request) {
            tracing::error!(%err, "Error sending renew key request.");
        }
    }
}

impl KeyManager {
    pub fn new(state_store: StateStore, sender: TypedSocketSender<RenewKeyRequest>) -> Self {
        Self {
            state_store,
            senders: HashMap::new(),
            sender,
        }
    }

    pub fn set_sender(&mut self, sender: TypedSocketSender<RenewKeyRequest>) {
        self.sender = sender;
        // todo: if a lock renewal was in-flight, it could get dropped. We need to
        // re-request it after a reconnect.
    }

    pub fn register_key(&mut self, key: AcquiredKey) {
        let (sender, receiver) = tokio::sync::watch::channel(key.clone());

        tokio::spawn(renew_key_loop(key.clone(), self.sender.clone(), receiver));

        self.senders.insert(key.key, sender);
    }

    pub fn unregister_key(&mut self, key: &KeyConfig) {
        self.senders.remove(key);
    }

    pub fn receive_response(&mut self, response: AcquiredKey) {
        if let Some(sender) = self.senders.get_mut(&response.key) {
            let _ = sender.send(response);
        } else {
            tracing::warn!(?response, "Received response for unknown key.");
        }
    }
}
