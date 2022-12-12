use crate::messages::agent::BackendState;
use std::collections::HashMap;
use time::OffsetDateTime;
use tokio::sync::mpsc::{error::TrySendError, Receiver, Sender};
use tokio_stream::Stream;

#[derive(Clone, Debug)]
struct Subscriptions<T> {
    subscriptions: HashMap<usize, Sender<T>>,
    next_id: usize,
}

impl<T> Default for Subscriptions<T> {
    fn default() -> Self {
        Self {
            subscriptions: HashMap::new(),
            next_id: 0,
        }
    }
}

impl<T: Clone> Subscriptions<T> {
    fn subscribe(&mut self) -> Receiver<T> {
        let (sender, receiver) = tokio::sync::mpsc::channel(10);
        let id = self.next_id;
        self.next_id += 1;
        self.subscriptions.insert(id, sender);
        receiver
    }

    /// Send a message to all subscribers, and remove any whose receiver has been dropped.
    fn send(&mut self, message: T) {
        self.subscriptions.retain(|_, sender| {
            match sender.try_send(message.clone()) {
                Ok(_) => true,
                Err(TrySendError::Closed(_)) => {
                    // The receiver has been dropped, so we drop the sender.
                    false
                }
                Err(TrySendError::Full(_)) => {
                    tracing::error!("Subscription is full, dropping it. This should never happen.");
                    true
                }
            }
        });
    }
}

#[derive(Default, Clone, Debug)]
pub struct BackendView {
    states: Vec<(BackendState, OffsetDateTime)>,
    subscriptions: Subscriptions<(BackendState, OffsetDateTime)>,
}

impl BackendView {
    pub fn state(&self) -> Option<BackendState> {
        let (last_backend_state, _timestamp) = self.states.last()?;
        Some(*last_backend_state)
    }

    pub fn update_state(&mut self, state: BackendState, timestamp: OffsetDateTime) {
        self.states.push((state, timestamp));
        self.subscriptions.send((state, timestamp));
    }

    pub fn stream(&mut self) -> impl Stream<Item = (BackendState, OffsetDateTime)> {
        let states = self.states.clone();
        let mut subscription = self.subscriptions.subscribe();
        Box::pin(async_stream::stream! {
            for state in states {
                yield state;
            }
            while let Some((state, timestamp)) = subscription.recv().await {
                yield (state, timestamp);
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn test_backend_view() {
        let mut backend_view = BackendView::default();
        assert_eq!(backend_view.state(), None);

        let mut stream = backend_view.stream();

        backend_view.update_state(
            BackendState::Ready,
            OffsetDateTime::from_unix_timestamp(5).unwrap(),
        );
        let (state, timestamp) = timeout(Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, BackendState::Ready);
        assert_eq!(timestamp, OffsetDateTime::from_unix_timestamp(5).unwrap());

        assert_eq!(backend_view.state(), Some(BackendState::Ready));

        backend_view.update_state(
            BackendState::Swept,
            OffsetDateTime::from_unix_timestamp(10).unwrap(),
        );
        let (state, timestamp) = timeout(Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, BackendState::Swept);
        assert_eq!(timestamp, OffsetDateTime::from_unix_timestamp(10).unwrap());

        assert_eq!(backend_view.state(), Some(BackendState::Swept));

        // Test that the stream initially returns all the states before returning new ones.
        let mut stream = backend_view.stream();
        assert_eq!(
            timeout(Duration::from_secs(3), stream.next())
                .await
                .unwrap()
                .unwrap(),
            (
                BackendState::Ready,
                OffsetDateTime::from_unix_timestamp(5).unwrap()
            )
        );
        assert_eq!(
            timeout(Duration::from_secs(3), stream.next())
                .await
                .unwrap()
                .unwrap(),
            (
                BackendState::Swept,
                OffsetDateTime::from_unix_timestamp(10).unwrap()
            )
        );
    }
}
