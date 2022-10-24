//! Typed wrappers around NATS.
//!
//! These use serde to serialize data to/from JSON over nats into Rust types.

use anyhow::{anyhow, Result};
use async_nats::jetstream;
use async_nats::jetstream::consumer::push::Messages;
use async_nats::jetstream::consumer::DeliverPolicy;
use async_nats::jetstream::stream::Config;
use async_nats::jetstream::Context;
use async_nats::{Client, Message, Subscriber};
use bytes::Bytes;
use dashmap::DashSet;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::error::Error;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;
use tokio_stream::StreamExt;

use crate::logging::LogError;

/// Unconstructable type, used as a [TypedMessage::Response] to indicate that
/// no response is allowed.
#[derive(Serialize, Deserialize)]
pub enum NoReply {}

pub trait TypedMessage: Serialize + DeserializeOwned {
    type Response: Serialize + DeserializeOwned;

    /// Returns the subject associated with this message.
    /// Subjects must be deterministically generated from the message
    /// body.
    fn subject(&self) -> String;
}

pub trait JetStreamable: TypedMessage {
    /// Returns the name of the JetStream associated with this message type.
    fn stream_name() -> &'static str;

    /// Returns the JetStream configuration associated with this message type.
    fn config() -> Config;
}

impl<M> Debug for SubscribeSubject<M>
where
    M: TypedMessage,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.subject.fmt(f)
    }
}

/// Wraps a NATS subject string with some type information about the message
/// type expected on that subject, as well as the reply type (which may be [NoReply]).
pub struct SubscribeSubject<M>
where
    M: TypedMessage,
{
    subject: String,
    _ph_m: PhantomData<M>,
}

impl<M> SubscribeSubject<M>
where
    M: TypedMessage,
{
    #[must_use]
    pub fn new(subject: String) -> SubscribeSubject<M> {
        SubscribeSubject {
            subject,
            _ph_m: PhantomData::default(),
        }
    }
}

#[derive(Debug)]
pub struct MessageWithResponseHandle<T>
where
    T: TypedMessage,
{
    /// Deserialized value of the message.
    pub value: T,
    /// Raw NATS message.
    message: Message,
    /// Handle to NATS client, retained for responding.
    nc: Client,
}

impl<T> MessageWithResponseHandle<T>
where
    T: TypedMessage,
{
    fn new(message: Message, nc: Client) -> Result<Self> {
        Ok(MessageWithResponseHandle {
            value: serde_json::from_slice(&message.payload)?,
            message,
            nc,
        })
    }

    pub fn message(&self) -> &Message {
        &self.message
    }

    pub async fn respond(&self, response: &T::Response) -> Result<()> {
        self.nc
            .publish(
                self.message
                    .reply
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow!("Attempted to respond to a message with no reply subject.")
                    })?
                    .to_string(),
                Bytes::from(serde_json::to_vec(response)?),
            )
            .await?;
        Ok(())
    }
}

pub struct TypedSubscription<T>
where
    T: TypedMessage,
{
    subscription: Subscriber,
    nc: Client,
    _ph_t: PhantomData<T>,
}

impl<T> TypedSubscription<T>
where
    T: TypedMessage,
{
    fn new(subscription: Subscriber, nc: Client) -> Self {
        TypedSubscription {
            subscription,
            nc,
            _ph_t: PhantomData::default(),
        }
    }

    pub async fn next(&mut self) -> Option<MessageWithResponseHandle<T>> {
        loop {
            if let Some(message) = self.subscription.next().await {
                let result = MessageWithResponseHandle::new(
                    message,
                    self.nc.clone(),
                );
                match result {
                    Ok(v) => return Some(v),
                    Err(error) => tracing::error!(?error, "Error parsing message; message ignored."),
                }
            } else {
                return None
            }
        }
    }
}

/// NATS errors are not castable to anyhow::Error, because they don't
/// implement [Sized] for some reason.
///
/// This helper trait is used to add some convenience helpers to
/// `Result<_, async_nats::Error>` to make it easy to convert these
/// to [anyhow::Error] errors.
trait NatsResultExt<T> {
    fn to_anyhow(self) -> Result<T>;

    fn with_message(self, message: &'static str) -> Result<T>;
}

impl<T> NatsResultExt<T> for std::result::Result<T, async_nats::Error> {
    fn with_message(self, message: &'static str) -> Result<T> {
        match self {
            Ok(v) => Ok(v),
            Err(err) => Err(anyhow!("NATS Error: {:?} ({})", err, message)),
        }
    }

    fn to_anyhow(self) -> Result<T> {
        match self {
            Ok(v) => Ok(v),
            Err(err) => Err(anyhow!("NATS Error: {:?}", err)),
        }
    }
}

#[derive(Clone)]
pub struct TypedNats {
    nc: Client,
    jetstream: Context,
    /// A set of JetStream names which have been created by this client.
    /// JetStreams are lazily created by TypedNats the first time they
    /// are used, and then stored here to avoid a round-trip after that.
    /// Stream creation (with the same config) is idempotent, so it doesn't
    /// matter if this is called multiple times, but we want to avoid
    /// creating the same stream repeatedly because it costs a round-trip
    /// to NATS.
    jetstream_created_streams: Arc<DashSet<String>>,
}

pub struct DelayedReply<T: DeserializeOwned> {
    subscription: Subscriber,
    _ph: PhantomData<T>,
}

impl<T: DeserializeOwned> DelayedReply<T> {
    pub async fn response(&mut self) -> Result<T> {
        let message = self
            .subscription
            .next()
            .await
            .ok_or_else(|| anyhow!("Expected response."))?;

        Ok(serde_json::from_slice(&message.payload)?)
    }
}

pub struct JetstreamSubscription<T: TypedMessage> {
    stream: Messages,
    _ph: PhantomData<T>,
}

impl<T: TypedMessage> JetstreamSubscription<T> {
    pub async fn next(&mut self) -> Option<T> {
        loop {
            if let Some(message) = self.stream.next().await {
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::error!(?error, "Error accessing jetstream message.");
                        continue;
                    }
                };
                message.ack().await.log_error("Error acking jetstream message.");
                let value: Result<T, _> = serde_json::from_slice(&message.payload);
                match value {
                    Ok(value) => return Some(value),
                    Err(error) => tracing::error!(?error, "Error parsing jetstream message; message ignored."),
                }
            } else {
                return None
            }
        }
    }
}

/// async_nats returns an Ok(Err(_)) when a stream is empty instead of None, this replaces that specific error
/// with None.
///
/// This will not be necessary once async_nats has concrete error types, which is coming.
fn nats_error_hack(
    message_result: Option<Result<jetstream::Message, Box<dyn Error + Send + Sync>>>,
) -> anyhow::Result<Option<jetstream::Message>> {
    match message_result {
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(err)) => {
            // If we update async_nats to a version that includes https://github.com/nats-io/nats.rs/pull/652, correct the typo below.
            if err.to_string()
                == r#"eror while processing messages from the stream: 404, Some("No Messages")"#
            {
                return Ok(None);
            }
            Err(anyhow!("NATS Error: {:?}", err))
        }
        None => Ok(None),
    }
}

impl TypedNats {
    #[must_use]
    pub fn new(nc: Client) -> Self {
        let jetstream = async_nats::jetstream::new(nc.clone());
        TypedNats {
            nc,
            jetstream,
            jetstream_created_streams: Arc::default(),
        }
    }

    pub async fn ensure_jetstream_exists<T: JetStreamable>(&self) -> Result<()> {
        if !self.jetstream_created_streams.contains(T::stream_name()) {
            self.add_jetstream_stream::<T>().await?;

            self.jetstream_created_streams
                .insert(T::stream_name().to_string());
        }

        Ok(())
    }

    /// Send a request that expects a reply, but return as soon as the request
    /// is sent with a handle that can later be awaited for the result.
    pub async fn split_request<T>(&self, message: &T) -> Result<DelayedReply<T::Response>>
    where
        T: TypedMessage,
    {
        let inbox = self.nc.new_inbox();
        let subscription = self.nc.subscribe(inbox.clone()).await.to_anyhow()?;
        self.nc
            .publish_with_reply(
                message.subject(),
                inbox,
                Bytes::from(serde_json::to_vec(&message)?),
            )
            .await
            .to_anyhow()?;

        Ok(DelayedReply {
            subscription,
            _ph: PhantomData::default(),
        })
    }

    async fn add_jetstream_stream<T: JetStreamable>(&self) -> Result<()> {
        let config = T::config();
        tracing::debug!(name = config.name, "Creating jetstream stream.");
        self.jetstream
            .get_or_create_stream(config)
            .await
            .to_anyhow()?;

        Ok(())
    }

    pub async fn get_all<T>(
        &self,
        subject: &SubscribeSubject<T>,
        deliver_policy: DeliverPolicy,
    ) -> Result<Vec<T>>
    where
        T: TypedMessage<Response = NoReply> + JetStreamable,
    {
        let _ = self.ensure_jetstream_exists::<T>().await;
        let stream = self
            .jetstream
            .get_stream(T::stream_name())
            .await
            .to_anyhow()?;

        let consumer = stream
            .create_consumer(async_nats::jetstream::consumer::pull::Config {
                deliver_policy,
                filter_subject: subject.subject.clone(),
                ..async_nats::jetstream::consumer::pull::Config::default()
            })
            .await
            .to_anyhow()?;

        let mut result: Vec<T> = Vec::new();

        loop {
            let mut messages = consumer.fetch().messages().await.to_anyhow()?;
            let mut done = true;

            while let Some(v) = nats_error_hack(messages.next().await)? {
                done = false;

                result.push(serde_json::from_slice(&v.payload)?);
            }

            if done {
                break;
            }
        }

        Ok(result)
    }

    /// Returns an ORDERED stream of messages published to a nats topic
    pub async fn subscribe_jetstream<T: JetStreamable>(
        &self,
        subject: SubscribeSubject<T>,
    ) -> Result<JetstreamSubscription<T>> {
        let subject = subject.subject.to_string();
        let _ = self.ensure_jetstream_exists::<T>().await;
        let stream_name = T::stream_name();

        let stream = self.jetstream.get_stream(stream_name).await.to_anyhow()?;
        let deliver_subject = self.nc.new_inbox();

        let consumer = stream
            .create_consumer(async_nats::jetstream::consumer::push::Config {
                deliver_policy: DeliverPolicy::All,
                filter_subject: subject,
                deliver_subject,
                max_ack_pending: 1, // NOTE: If you remove this or change the value,
                // the resultant stream is no longer guaranteed to be in order, and call sites
                // that rely on ordered messages will break, nondeterministically
                ..Default::default()
            })
            .await
            .to_anyhow()?;

        let stream: Messages = consumer.messages().await.to_anyhow()?;

        Ok(JetstreamSubscription {
            stream,
            _ph: PhantomData::default(),
        })
    }

    pub async fn publish<T>(&self, value: &T) -> Result<()>
    where
        T: TypedMessage<Response = NoReply>,
    {
        self.nc
            .publish(
                value.subject().clone(),
                Bytes::from(serde_json::to_vec(value)?),
            )
            .await?;
        Ok(())
    }

    pub async fn publish_jetstream<T>(&self, value: &T) -> Result<()>
    where
        T: TypedMessage<Response = NoReply> + JetStreamable,
    {
        self.ensure_jetstream_exists::<T>().await?;

        self.jetstream
            .publish(
                value.subject().clone(),
                Bytes::from(serde_json::to_vec(value)?),
            )
            .await
            .to_anyhow()?;
        Ok(())
    }

    pub async fn request<T>(&self, value: &T) -> Result<T::Response>
    where
        T: TypedMessage,
    {
        let result = self
            .nc
            .request(value.subject(), Bytes::from(serde_json::to_vec(value)?))
            .await
            .to_anyhow()?;

        let value: T::Response = serde_json::from_slice(&result.payload)?;
        Ok(value)
    }

    pub async fn subscribe<T>(&self, subject: SubscribeSubject<T>) -> Result<TypedSubscription<T>>
    where
        T: TypedMessage,
    {
        let subscription = self.nc.subscribe(subject.subject).await.to_anyhow()?;
        Ok(TypedSubscription::new(subscription, self.nc.clone()))
    }
}
