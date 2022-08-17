use std::{future::Future, sync::mpsc::{Sender, Receiver, channel}, pin::Pin};

pub mod container;
pub mod system;

thread_local! {
    pub static TEARDOWN_TASK_MANAGER: TeardownTaskManager = TeardownTaskManager::new();
}

pub type BoxedFuture = Pin<Box<dyn Future<Output=Result<(), anyhow::Error>>>>;

pub struct TeardownTaskManager {
    sender: Sender<BoxedFuture>,
    receiver: Receiver<BoxedFuture>,
}

impl TeardownTaskManager {
    pub fn new() -> Self {
        let (sender, receiver) = channel();

        TeardownTaskManager { sender, receiver }
    }

    pub fn add_task<T>(&self, task: T) where T: Future<Output=Result<(), anyhow::Error>> + 'static {
        self.sender.send(Box::pin(task));
    }

    pub async fn teardown(&self) {
        while let Ok(task) = self.receiver.try_recv() {
            if let Err(e) = task.await {
                tracing::error!(?e, "Error encountered during teardown.");
            }
        }
    }
}