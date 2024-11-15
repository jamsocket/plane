use crate::database::{backend::BackendMetricsMessage, util::MapSqlxError};
use crate::util::ExponentialBackoff;
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sqlx::{postgres::PgListener, PgConnection, PgPool};
use std::{
    any::Any,
    collections::HashMap,
    fmt::Debug,
    sync::{Arc, RwLock},
};
use tokio::{
    sync::broadcast::{Receiver, Sender},
    task::JoinHandle,
};

type ListenerMap = Arc<RwLock<HashMap<(String, Option<String>), Box<dyn TypedSender>>>>;

pub const EVENT_CHANNEL: &str = "plane_events";
pub const BACKEND_METRICS_EVENT_CHANNEL: &str = "plane_backend_metrics";

pub trait NotificationPayload:
    Serialize + DeserializeOwned + Debug + Send + Sync + Clone + 'static
{
    fn kind() -> &'static str;
}

trait TypedSender: Send + Sync {
    fn send(&self, value: Notification<Value>);

    fn receiver_count(&self) -> usize;

    fn as_any(&self) -> &dyn Any;
}

impl<T: NotificationPayload> TypedSender for Sender<Notification<T>> {
    fn send(&self, value: Notification<Value>) {
        let payload: T = match serde_json::from_value(value.payload) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::error!(?err, "Failed to deserialize notification payload.");
                return;
            }
        };

        let value = Notification {
            id: value.id,
            timestamp: value.timestamp,
            kind: value.kind,
            key: value.key,
            payload,
        };

        if let Err(err) = Sender::send(self, value) {
            tracing::error!(?err, "Failed to send notification.");
        }
    }

    fn receiver_count(&self) -> usize {
        Sender::receiver_count(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct Subscription<T: Clone> {
    receiver: Option<Receiver<Notification<T>>>,
    table: ListenerMap,
    key: (String, Option<String>),
}

impl<T: Clone> Subscription<T> {
    pub async fn next(&mut self) -> Option<Notification<T>> {
        self.receiver
            .as_mut()
            .expect("Receiver can't be taken until subscription is dropped.")
            .recv()
            .await
            .ok()
    }
}

impl<T: Clone> Drop for Subscription<T> {
    fn drop(&mut self) {
        let receiver = self
            .receiver
            .take()
            .expect("Receiver can't be taken until dropped.");
        drop(receiver);

        let mut table = self.table.write().expect("Table lock is poisoned.");

        // If after dropping the receiver, there are no more receivers for this key, remove the
        // entry from the table.
        if let Some(sender) = table.get_mut(&self.key) {
            if sender.receiver_count() == 0 {
                table.remove(&self.key);
            }
        } else {
            tracing::warn!("Subscription dropped but no associated sender found in table.");
        }
    }
}

pub struct EventSubscriptionManager {
    all_events: Sender<Notification<Value>>,
    handle: JoinHandle<()>,

    /// Maps from (kind, optional_key) to a sender.
    listeners: ListenerMap,
}

impl Drop for EventSubscriptionManager {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Notification<T> {
    /// Optional id. If not present, the notification is ephemeral.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i32>,

    pub timestamp: DateTime<Utc>,

    /// Rust type of the payload.
    pub kind: String,

    /// Optional key to identify the payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// The payload.
    pub payload: T,
}

impl EventSubscriptionManager {
    pub fn new(db: &PgPool) -> Self {
        let listeners: ListenerMap = Arc::new(RwLock::new(HashMap::new()));
        let all_events = Sender::new(1000);

        let handle = {
            let all_events = all_events.clone();
            let listeners = listeners.clone();
            let db = db.clone();

            tokio::spawn(async move {
                let mut backoff = ExponentialBackoff::default();
                let mut last_message: Option<i32> = None;

                let send_message = move |notification: Notification<Value>| {
                    if all_events.receiver_count() > 0 {
                        let _ = all_events.send(notification.clone());
                    }

                    let listeners = listeners.read().expect("Listener map is poisoned.");

                    // If the notification has a key, we send it both to the global listeners and
                    // the listeners for the specific key.
                    if let Some(key) = notification.key.as_ref() {
                        if let Some(sender) =
                            listeners.get(&(notification.kind.clone(), Some(key.clone())))
                        {
                            sender.send(notification.clone());
                        }
                    }

                    // Send the notification to the global listeners.
                    if let Some(sender) = listeners.get(&(notification.kind.clone(), None)) {
                        sender.send(notification);
                    }
                };

                'outer: loop {
                    let mut listener = match PgListener::connect_with(&db).await {
                        Ok(listener) => listener,
                        Err(err) => {
                            tracing::error!(?err, "Failed to connect to database.");
                            backoff.wait().await;
                            continue;
                        }
                    };

                    if let Err(err) = listener.listen(EVENT_CHANNEL).await {
                        tracing::error!(?err, "Failed to listen to event channel.");
                        backoff.wait().await;
                        continue;
                    }

                    if let Some(mut prev_last_message) = last_message {
                        'inner: loop {
                            let messages = match EventSubscriptionManager::get_events_since(
                                &db,
                                prev_last_message,
                            )
                            .await
                            {
                                Ok(messages) => messages,
                                Err(err) => {
                                    tracing::error!(
                                        ?err,
                                        "Failed to fetch messages from database."
                                    );
                                    backoff.wait().await;
                                    continue 'outer;
                                }
                            };

                            if messages.is_empty() {
                                break 'inner;
                            }

                            for message in messages {
                                send_message(message.clone());
                                last_message = message.id;
                                prev_last_message = message
                                    .id
                                    .expect("Expected message from database to have id.");
                            }
                        }
                    }

                    backoff.defer_reset();

                    while let Ok(Some(notification)) = listener.try_recv().await {
                        let notification: Notification<Value> =
                            match serde_json::from_str(notification.payload()) {
                                Ok(notification) => notification,
                                Err(err) => {
                                    tracing::error!(?err, "Failed to deserialize notification.");
                                    continue;
                                }
                            };

                        // It could happen that a message happens between when we open the listener
                        // and when we request messages from the DB. In that case, we don't want to
                        // send the message twice. If the message has no id, it's ephemeral and we
                        // should send it since it wouldn't have been in the DB anyway.
                        if let Some(notification_id) = notification.id {
                            if let Some(last_message) = last_message {
                                if notification_id <= last_message {
                                    continue;
                                }
                            }
                            last_message = Some(notification_id);
                        }
                        send_message(notification);
                    }

                    tracing::error!("Lost connection to database, reconnecting after wait.");
                    backoff.wait().await;
                }
            })
        };

        Self {
            all_events,
            handle,
            listeners,
        }
    }

    pub async fn clean_up_events(db: &PgPool, min_age_days: i32) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            delete from event
            where created_at < now() - make_interval(days => $1)
            "#,
            min_age_days
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_events_since(
        db: &PgPool,
        since: i32,
    ) -> Result<Vec<Notification<Value>>, sqlx::Error> {
        let messages = sqlx::query!(
            r#"
            select id, kind, key, data, created_at
            from event
            where id > $1
            order by id asc
            limit 100
            "#,
            since,
        )
        .fetch_all(db)
        .await?;

        Ok(messages
            .into_iter()
            .map(|message| Notification {
                id: Some(message.id),
                timestamp: message.created_at,
                kind: message.kind,
                key: message.key,
                payload: message.data,
            })
            .collect())
    }

    pub fn subscribe_all_events(&self) -> Receiver<Notification<Value>> {
        self.all_events.subscribe()
    }

    pub fn subscribe<T: NotificationPayload>(&self, key: Option<&str>) -> Subscription<T> {
        let kind = T::kind().to_string();

        let mut listeners = self.listeners.write().expect("Listener map is poisoned.");
        let key = (kind.clone(), key.map(|s| s.to_string()));

        match listeners.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let sender = entry
                    .get()
                    .as_any()
                    .downcast_ref::<Sender<Notification<T>>>()
                    .expect(
                        "Sender is not of the expected type. \
                        This implies that two different types return the same value \
                        for NotificationPayload::kind(), which is not allowed.",
                    );
                Subscription {
                    receiver: Some(sender.subscribe()),
                    table: self.listeners.clone(),
                    key,
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let sender = Sender::new(1000);
                let receiver = sender.subscribe();
                entry.insert(Box::new(sender));
                Subscription {
                    receiver: Some(receiver),
                    table: self.listeners.clone(),
                    key,
                }
            }
        }
    }
}

pub async fn emit_impl<T: NotificationPayload>(
    db: &mut PgConnection,
    key: Option<&str>,
    payload: &T,
) -> Result<(), sqlx::Error> {
    let kind = T::kind().to_string();
    sqlx::query!(
        r#"
        with message_insert as (
            insert into event (kind, key, created_at, data)
            values ($1, $2, now(), $3)
            returning id
        )
        select pg_notify(
            $4,
            json_build_object(
                'payload', $3::jsonb,
                'timestamp', now(),
                'id', id,
                'kind', $1,
                'key', $2
            )::text
        ) from message_insert
        "#,
        kind,
        key,
        serde_json::to_value(&payload).map_sqlx_error()?,
        EVENT_CHANNEL,
    )
    .execute(&mut *db)
    .await?;

    Ok(())
}

pub async fn emit<T: NotificationPayload>(
    db: &mut PgConnection,
    payload: &T,
) -> Result<(), sqlx::Error> {
    emit_impl(db, None, payload).await
}

pub async fn emit_with_key<T: NotificationPayload>(
    db: &mut PgConnection,
    key: &str,
    payload: &T,
) -> Result<(), sqlx::Error> {
    emit_impl(db, Some(key), payload).await
}

pub async fn emit_backend_metrics(
    db: &mut PgConnection,
    key: &str,
    payload: &BackendMetricsMessage,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        select pg_notify(
            $4,
            json_build_object(
                'payload', $3::jsonb,
                'timestamp', now(),
                'kind', $1::text,
                'key', $2::text
            )::text
        )"#,
        BackendMetricsMessage::kind().to_string(),
        key,
        serde_json::to_value(&payload).map_sqlx_error()?,
        BACKEND_METRICS_EVENT_CHANNEL,
    )
    .execute(db)
    .await?;

    Ok(())
}
