//! Typed wrappers around NATS.
//!
//! These use serde to serialize data to/from JSON over nats into Rust types.

use anyhow::{anyhow, Result};
use futures::channel::oneshot::channel;
use futures::Stream;
use nats::asynk::{Connection, Message, Subscription};
use nats::jetstream::{JetStream, StreamConfig, SubscribeOptions};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::marker::PhantomData;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Serialize, Deserialize)]
pub enum NoReply {}

#[derive(Clone, Debug)]
pub struct Subject<M, R>
where
    M: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    subject: String,
    _ph_m: PhantomData<M>,
    _ph_r: PhantomData<R>,
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
    _ph: PhantomData<R>,
}

impl<T, R> MessageWithResponseHandle<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn new(message: Message) -> Result<Self> {
        Ok(MessageWithResponseHandle {
            value: serde_json::from_slice(&message.data)?,
            message,
            _ph: PhantomData::default(),
        })
    }

    pub async fn respond(&self, response: &R) -> Result<()> {
        Ok(self.message.respond(&serde_json::to_vec(response)?).await?)
    }
}

pub struct TypedSubscription<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    subscription: Subscription,
    _ph_t: PhantomData<T>,
    _ph_r: PhantomData<R>,
}

impl<T, R> TypedSubscription<T, R>
where
    T: Serialize + DeserializeOwned,
    R: Serialize + DeserializeOwned,
{
    fn new(subscription: Subscription) -> Self {
        TypedSubscription {
            subscription,
            _ph_r: PhantomData::default(),
            _ph_t: PhantomData::default(),
        }
    }

    pub async fn next(&mut self) -> Result<Option<MessageWithResponseHandle<T, R>>> {
        if let Some(message) = self.subscription.next().await {
            Ok(Some(MessageWithResponseHandle::new(message)?))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone)]
pub struct TypedNats {
    nc: Connection,
    jetstream: JetStream,
}

impl TypedNats {
    pub fn add_jetstream_stream<P, T, R>(&self, stream_name: &str, subject: P) -> Result<()>
    where
        P: Subscribable<T, R>,
        T: Serialize + DeserializeOwned,
        R: Serialize + DeserializeOwned,
    {
        self.jetstream.add_stream(StreamConfig {
            name: stream_name.to_string(),
            subjects: vec![subject.subject().to_string()],
            ..StreamConfig::default()
        })?;

        Ok(())
    }

    pub async fn connect(nats_url: &str) -> Result<Self> {
        let nc = nats::asynk::connect(nats_url).await?;
        let jetstream = nats::jetstream::new(nats::connect(nats_url)?);

        Ok(Self::new(nc, jetstream))
    }

    pub fn new(nc: Connection, jetstream: JetStream) -> Self {
        TypedNats { nc, jetstream }
    }

    pub async fn get_latest<T>(&self, subject: &Subject<T, NoReply>) -> Result<Option<T>>
    where
        T: Serialize + DeserializeOwned,
    {
        let subscription = self
            .jetstream
            .subscribe_with_options(subject.subject(), &SubscribeOptions::new().deliver_last())?;

        let (send, recv) = channel();
        tokio::task::spawn_blocking(move || {
            let result = subscription.next_timeout(Duration::from_secs(1));
            send.send(result)
        });

        let result = recv
            .await
            .map_err(|_| anyhow!("Error receiving from channel."))?;

        if let Ok(result) = result {
            Ok(Some(serde_json::from_slice(&result.data)?))
        } else {
            Ok(None)
        }
    }

    pub async fn subscribe_jetstream<P, T, R>(&self, subject: P) -> impl Stream<Item = T>
    where
        P: Subscribable<T, R>,
        T: Serialize + DeserializeOwned + Send + 'static,
        R: Serialize + DeserializeOwned,
    {
        let jetstream = self.jetstream.clone();
        let subject = subject.subject().to_string();

        // nats_rs does not yet have async support (https://github.com/nats-io/nats.rs/issues/194),
        // so this is less elegant than it could be.
        // The approach is to create a multiple-producer, single-consumer (mpsc) channel,
        // and run a loop in a tokio thread designated to allow blocking.
        // Technically, this could be a oneshot channel rather than mpsc, since we only ever
        // have one sender. But tokio ships with a convenient `ReceiverStream` that wraps
        // an mpsc receiver and implements `Stream`, so we use that approach for now.
        let (tx, rx) = mpsc::channel(256);
        tokio::task::spawn_blocking(move || {
            let subscription = match jetstream.subscribe(&subject) {
                Ok(subscription) => subscription,
                Err(error) => {
                    tracing::error!(?error, "Error subscribing to backend status.");
                    return;
                }
            };

            for message in subscription.iter() {
                if let Err(error) = message.ack() {
                    tracing::warn!(%error, "Error acknowledging backend status message.");
                }

                let v: T = match serde_json::from_slice(&message.data) {
                    Ok(v) => v,
                    Err(error) => {
                        tracing::error!(?error, "Couldn't parse backend status.");
                        continue;
                    }
                };

                let _ = tx.blocking_send(v);
            }
        });

        // TODO: is the connection leaked?
        // We probably want to keep track of the `JoinHandle` created by `spawn_blocking`,
        // and call .abort() on it when the stream is dropped. This will require implementing
        // an alternative to tokio's `ReceiverStream`.
        tokio_stream::wrappers::ReceiverStream::new(rx)
    }

    pub async fn publish<T>(&self, subject: &Subject<T, NoReply>, value: &T) -> Result<()>
    where
        T: Serialize + DeserializeOwned,
    {
        self.nc
            .publish(&subject.subject, serde_json::to_vec(value)?)
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
            .request(&subject.subject, serde_json::to_vec(value)?)
            .await?;

        let value: R = serde_json::from_slice(&result.data)?;
        Ok(value)
    }

    pub async fn subscribe<P, T, R>(&self, subject: P) -> Result<TypedSubscription<T, R>>
    where
        P: Subscribable<T, R>,
        T: Serialize + DeserializeOwned,
        R: Serialize + DeserializeOwned,
    {
        let subscription = self.nc.subscribe(subject.subject()).await?;
        Ok(TypedSubscription::new(subscription))
    }
}
