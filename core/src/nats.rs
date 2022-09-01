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
use futures::{Stream, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::time::Duration;
use tokio::time::timeout;

use crate::messages::create_streams;

#[derive(Serialize, Deserialize)]
pub enum NoReply {}

pub trait TypedMessage: Serialize + DeserializeOwned {
    type Response: Serialize + DeserializeOwned;

    fn subject(&self) -> String;
}

pub trait JetStreamable: TypedMessage {
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
    pub value: T,
    message: Message,
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

    pub async fn add_jetstream_stream<T: JetStreamable>(&self) -> Result<()> {
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
        // Ensure that streams exist.
        create_streams(&result).await?;

        Ok(result)
    }

    #[must_use]
    pub fn new(nc: Client) -> Self {
        let jetstream = async_nats::jetstream::new(nc.clone());
        TypedNats { nc, jetstream }
    }

    pub async fn get_latest<T>(
        &self,
        subject: &SubscribeSubject<T>,
        stream_name: &str,
    ) -> Result<Option<T>>
    where
        T: TypedMessage<Response = NoReply>,
    {
        let stream = self.jetstream.get_stream(stream_name).await.to_anyhow()?;

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
        let config = T::config();
        let stream_name = config.name;

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
