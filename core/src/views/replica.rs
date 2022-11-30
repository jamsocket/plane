use super::SystemView;
use crate::nats::JetstreamSubscription;
use crate::{messages::state::StateUpdate, nats::TypedNats, NeverResult};
use anyhow::{anyhow, Result};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use tokio::task::JoinHandle;

pub struct SystemViewReplica {
    view: Arc<RwLock<SystemView>>,
    handle: JoinHandle<NeverResult>,
}

impl SystemViewReplica {
    async fn snapshot_impl(nats: TypedNats) -> Result<(SystemView, JetstreamSubscription<StateUpdate>)> {
        let mut view = SystemView::default();
        let mut sub = nats.subscribe_jetstream::<StateUpdate>().await?;

        if sub.has_pending {
            while let Some((update, meta)) = sub.next().await {
                view.update_state(update, meta.timestamp);
    
                if !sub.has_pending {
                    break;
                }
            }
        }            

        Ok((view, sub))
    }

    pub async fn snapshot(nats: TypedNats) -> Result<SystemView> {
        Ok(Self::snapshot_impl(nats).await?.0)
    }

    pub async fn new(nats: TypedNats) -> Result<SystemViewReplica> {
        let (view, mut sub) = Self::snapshot_impl(nats).await?;
        let view = Arc::new(RwLock::new(view));

        let handle = {
            let view = view.clone();

            tokio::spawn(async move {
                while let Some((update, meta)) = sub.next().await {
                    view.write()
                        .expect("SystemView RwLock was poisoned.")
                        .update_state(update, meta.timestamp);
                }

                Err(anyhow!("SystemViewReplica loop terminated unexpectedly."))
            })
        };

        Ok(SystemViewReplica { view, handle })
    }

    pub fn view(&self) -> RwLockReadGuard<SystemView> {
        self.view.read().expect("SystemView RwLock was poisoned.")
    }
}

impl Drop for SystemViewReplica {
    fn drop(&mut self) {
        self.handle.abort()
    }
}
