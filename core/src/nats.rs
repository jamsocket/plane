//! Typed wrappers around NATS.
//!
//! These use serde to serialize data to/from JSON over nats into Rust types.

use anyhow::{anyhow, Result};
use async_nats::jetstream::consumer::DeliverPolicy;
use async_nats::jetstream::stream::Config;
use async_nats::jetstream::Context;
use async_nats::{Client, ConnectOptions, Message, Subscriber};
use async_stream::stream;
use bytes::Bytes;
use dashmap::DashSet;
use futures::{Stream, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

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

    pub async fn next(&mut self) -> Result<Option<MessageWithResponseHandle<T>>> {
        if let Some(message) = self.subscription.next().await {
            Ok(Some(MessageWithResponseHandle::new(
                message,
                self.nc.clone(),
            )?))
        } else {
            Ok(None)
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

impl TypedNats {
    async fn ensure_jetstream_exists<T: JetStreamable>(&self) -> Result<()> {
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
        self.jetstream
            .get_or_create_stream(config)
            .await
            .to_anyhow()?;

        Ok(())
    }

    pub async fn connect(nats_url: &str, options: ConnectOptions) -> Result<Self> {
        let nc = async_nats::connect_with_options(nats_url, options).await?;

        let result = Self::new(nc);

        Ok(result)
    }

    #[must_use]
    pub fn new(nc: Client) -> Self {
        let jetstream = async_nats::jetstream::new(nc.clone());
        TypedNats {
            nc,
            jetstream,
            jetstream_created_streams: Arc::default(),
        }
    }

    pub async fn get_latest<T>(&self, subject: &SubscribeSubject<T>) -> Result<Option<T>>
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
                deliver_policy: DeliverPolicy::Last,
                filter_subject: subject.subject.clone(),
                ..async_nats::jetstream::consumer::pull::Config::default()
            })
            .await
            .to_anyhow()?;

        let result = match timeout(
            Duration::from_secs(1),
            consumer.stream().messages().await.to_anyhow()?.next(),
        )
        .await
        {
            Ok(Some(v)) => v.to_anyhow()?,
            _ => return Ok(None),
        };

        Ok(Some(serde_json::from_slice(&result.payload)?))
    }

    pub async fn subscribe_jetstream<T: JetStreamable>(
        &self,
        subject: SubscribeSubject<T>,
    ) -> impl Stream<Item = T> {
        let jetstream = self.jetstream.clone();
        let subject = subject.subject.to_string();
        let _ = self.ensure_jetstream_exists::<T>().await;
        let stream_name = T::stream_name();

        let stream = stream!({
            let stream = jetstream.get_stream(stream_name).await.unwrap();

            let consumer = stream
                .create_consumer(async_nats::jetstream::consumer::pull::Config {
                    deliver_policy: DeliverPolicy::All,
                    filter_subject: subject,
                    ..async_nats::jetstream::consumer::pull::Config::default()
                })
                .await
                .unwrap();

            let mut stream = consumer.stream().messages().await.unwrap();

            while let Some(Ok(value)) = stream.next().await {
                let _ = value.ack().await;
                let value: Result<T, _> = serde_json::from_slice(&value.payload);
                match value {
                    Ok(value) => yield value,
                    Err(error) => tracing::warn!(?error, "Parse Err"),
                }
            }
        });

        stream
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
