use futures_util::Future;
use tokio::sync::broadcast;

pub struct ShutdownSignal {
    send_shutdown: broadcast::Sender<()>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        let (send_shutdown, _) = broadcast::channel::<()>(1);
        Self { send_shutdown }
    }

    pub fn shutdown(&self) {
        let _ = self.send_shutdown.send(());
    }

    pub fn subscribe(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut receiver = self.send_shutdown.subscribe();
        async move {
            let _ = receiver.recv().await;
        }
    }
}
