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

#[derive(Serialize, Deserialize)]
pub enum NoReply {}

#[derive(Clone)]
pub struct Subject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    subject: String,
    _ph_m: PhantomData<M>,
    _ph_r: PhantomData<R>,
}

impl<M, R> Debug for Subject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.subject.fmt(f)
    }
}

impl<M, R> Subject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    pub fn new(subject: String) -> Subject<M, R> {
        Subject {
            subject,
            _ph_m: PhantomData::default(),
            _ph_r: PhantomData::default(),
        }
    }
}

pub struct SubscribeSubject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    subject: String,
    _ph_m: PhantomData<M>,
    _ph_r: PhantomData<R>,
}

impl<M, R> SubscribeSubject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    pub fn new(subject: String) -> SubscribeSubject<M, R> {
        SubscribeSubject {
            subject,
            _ph_m: PhantomData::default(),
            _ph_r: PhantomData::default(),
        }
    }
}

pub trait Subscribable<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn subject(&self) -> &str;
}

impl<M, R> Subscribable<M, R> for Subject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn subject(&self) -> &str {
        &self.subject
    }
}

impl<M, R> Subscribable<M, R> for SubscribeSubject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn subject(&self) -> &str {
        &self.subject
    }
}

impl<M, R> Subscribable<M, R> for &Subject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn subject(&self) -> &str {
        &self.subject
    }
}

impl<M, R> Subscribable<M, R> for &SubscribeSubject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn subject(&self) -> &str {
        &self.subject
    }
}

#[derive(Debug)]
pub struct MessageWithResponseHandle<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    pub value: T,
    message: Message,
    nc: Client,
    _ph: PhantomData<R>,
}

impl<T, R> MessageWithResponseHandle<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn new(message: Message, nc: Client) -> Result<Self> {
        Ok(MessageWithResponseHandle {
            value: serde_json::from_slice(&message.payload)?,
            message,
            nc,
            _ph: PhantomData::default(),
        })
    }

    pub async fn respond(&self, response: &R) -> Result<()> {
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

pub struct TypedSubscription<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    subscription: Subscriber,
    nc: Client,
    _ph_t: PhantomData<T>,
    _ph_r: PhantomData<R>,
}

impl<T, R> TypedSubscription<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn new(subscription: Subscriber, nc: Client) -> Self {
        TypedSubscription {
            subscription,
            nc,
            _ph_r: PhantomData::default(),
            _ph_t: PhantomData::default(),
        }
    }

    pub async fn next(&mut self) -> Result<Option<MessageWithResponseHandle<T, R>>> {
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
    fn as_anyhow(self) -> Result<T>;

    fn with_message(self, message: &'static str) -> Result<T>;
}

impl<T> NatsResultExt<T> for std::result::Result<T, async_nats::Error> {
    fn with_message(self, message: &'static str) -> Result<T> {
        match self {
            Ok(v) => Ok(v),
            Err(err) => Err(anyhow!("NATS Error: {:?} ({})", err, message)),
        }
    }

    fn as_anyhow(self) -> Result<T> {
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

impl TypedNats {
    pub async fn add_jetstream_stream<P, T, R>(&self, stream_name: &str, subject: P) -> Result<()>
    where
        P: Subscribable<T, R>,
        T: Serialize + DeserializeOwned,
        R: Serialize + DeserializeOwned,
    {
        self.jetstream
            .get_or_create_stream(Config {
                name: stream_name.to_string(),
                subjects: vec![subject.subject().to_string()],
                ..Config::default()
            })
            .await
            .as_anyhow()?;

        Ok(())
    }

    pub async fn connect(nats_url: &str, options: ConnectOptions) -> Result<Self> {
        let nc = async_nats::connect_with_options(nats_url, options).await?;

        Ok(Self::new(nc))
    }

    pub fn new(nc: Client) -> Self {
        let jetstream = async_nats::jetstream::new(nc.clone());
        TypedNats { nc, jetstream }
    }

    pub async fn get_latest<T>(
        &self,
        subject: &Subject<T, NoReply>,
        stream_name: &str,
    ) -> Result<Option<T>>
    where
        T: Serialize + DeserializeOwned,
    {
        let stream = self.jetstream.get_stream(stream_name).await.as_anyhow()?;

        let consumer = stream
            .create_consumer(async_nats::jetstream::consumer::pull::Config {
                deliver_policy: DeliverPolicy::Last,
                filter_subject: subject.subject().to_string(),
                ..async_nats::jetstream::consumer::pull::Config::default()
            })
            .await
            .as_anyhow()?;

        let result = match timeout(
            Duration::from_secs(1),
            consumer.stream().messages().await.as_anyhow()?.next(),
        )
        .await
        {
            Ok(Some(v)) => v.as_anyhow()?,
            _ => return Ok(None),
        };

        Ok(Some(serde_json::from_slice(&result.payload)?))
    }

    pub async fn subscribe_jetstream<P, T, R>(
        &self,
        subject: P,
        stream_name: &str,
    ) -> impl Stream<Item = T>
    where
        P: Subscribable<T, R>,
        T: Serialize + DeserializeOwned + Send + 'static,
        R: Serialize + DeserializeOwned,
    {
        let jetstream = self.jetstream.clone();
        let subject = subject.subject().to_string();
        let stream_name = stream_name.to_string();

        let stream = stream!({
            let stream = jetstream.get_stream(stream_name.to_string()).await.unwrap();

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

    pub async fn publish<T>(&self, subject: &Subject<T, NoReply>, value: &T) -> Result<()>
    where
        T: Serialize + DeserializeOwned,
    {
        self.nc
            .publish(
                subject.subject.clone(),
                Bytes::from(serde_json::to_vec(value)?),
            )
            .await?;
        Ok(())
    }

    pub async fn request<T, R>(&self, subject: &Subject<T, R>, value: &T) -> Result<R>
    where
        T: Serialize + DeserializeOwned,
        R: Serialize + DeserializeOwned,
    {
        let result = self
            .nc
            .request(
                subject.subject.to_string(),
                Bytes::from(serde_json::to_vec(value)?),
            )
            .await
            .as_anyhow()?;

        let value: R = serde_json::from_slice(&result.payload)?;
        Ok(value)
    }

    pub async fn subscribe<P, T, R>(&self, subject: P) -> Result<TypedSubscription<T, R>>
    where
        P: Subscribable<T, R>,
        T: Serialize + DeserializeOwned,
        R: Serialize + DeserializeOwned,
    {
        let subscription = self
            .nc
            .subscribe(subject.subject().to_string())
            .await
            .as_anyhow()?;
        Ok(TypedSubscription::new(subscription, self.nc.clone()))
    }
}
