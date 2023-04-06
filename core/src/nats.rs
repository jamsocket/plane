//! Typed wrappers around NATS.
//!
//! These use serde to serialize data to/from JSON over nats into Rust types.

use crate::logging::LogError;
use anyhow::{anyhow, Context, Result};
use async_nats::jetstream;
use async_nats::jetstream::consumer::push::Messages;
use async_nats::jetstream::consumer::DeliverPolicy;
use async_nats::jetstream::context::Publish;
use async_nats::jetstream::stream::Config;
use async_nats::{Client, Message, Subscriber};
use bytes::Bytes;
use dashmap::DashSet;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio_stream::StreamExt;

/// This code is returned by NATS when an expected last sequence number is violated.
const NATS_WRONG_LAST_SEQUENCE_CODE: &str = "10071";

/// Unconstructable type, used as a [TypedMessage::Response] to indicate that
/// no response is allowed.
#[derive(Serialize, Deserialize)]
pub enum NoReply {}

pub struct MessageMeta {
    pub timestamp: OffsetDateTime,
    pub pending: u64,
    pub sequence: u64,
}

impl TryFrom<jetstream::Message> for MessageMeta {
    type Error = anyhow::Error;

    fn try_from(value: jetstream::Message) -> Result<Self, Self::Error> {
        let info = value.info().to_anyhow()?;

        Ok(MessageMeta {
            timestamp: info.published,
            pending: info.pending,
            sequence: info.stream_sequence,
        })
    }
}

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

/// Parse the given Message, and verify that the subject it arrived over is the
/// expected subject for that message.
fn parse_and_verify_message<T: TypedMessage>(message: &Message) -> Result<T> {
    let value: T = serde_json::from_slice(&message.payload)?;
    if value.subject() != message.subject {
        return Err(anyhow!(
            "Message subject ({}) does not match expected subject ({})",
            message.subject,
            value.subject()
        ));
    }

    Ok(value)
}

impl<T> MessageWithResponseHandle<T>
where
    T: TypedMessage,
{
    fn new(message: Message, nc: Client) -> Result<Self> {
        Ok(MessageWithResponseHandle {
            value: parse_and_verify_message(&message)?,
            message,
            nc,
        })
    }

    pub fn message(&self) -> &Message {
        &self.message
    }

    pub async fn try_respond(&self, response: &T::Response) -> Result<()> {
        if self.message.reply.is_some() {
            self.respond(response).await
        } else {
            Ok(())
        }
    }

    pub async fn respond(&self, response: &T::Response) -> Result<()> {
        let reply_inbox = self
            .message
            .reply
            .as_ref()
            .context("Attempted to respond to a message with no reply subject.")?
            .to_string();

        tracing::info!("Responding to message on {}", reply_inbox);

        self.nc
            .publish(reply_inbox, Bytes::from(serde_json::to_vec(response)?))
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
        while let Some(message) = self.subscription.next().await {
            let result = MessageWithResponseHandle::new(message, self.nc.clone());
            match result {
                Ok(v) => return Some(v),
                Err(error) => {
                    tracing::error!(?error, "Error parsing message; message ignored.")
                }
            }
        }

        None
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
    jetstream: jetstream::Context,
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
            .context("Expected response from DelayedReply.")?;

        Ok(serde_json::from_slice(&message.payload)?)
    }
}

pub struct JetstreamSubscription<T: TypedMessage> {
    stream: Messages,

    /// True if this consumer has pending messages to be consumed.
    pub has_pending: bool,
    _ph: PhantomData<T>,
}

impl<T: TypedMessage> JetstreamSubscription<T> {
    pub async fn next(&mut self) -> Option<(T, MessageMeta)> {
        loop {
            if let Some(message) = self.stream.next().await {
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::error!(?error, "Error accessing jetstream message.");
                        continue;
                    }
                };
                message
                    .ack()
                    .await
                    .log_error("Error acking jetstream message.");
                let value: Result<T, _> = parse_and_verify_message(&message);
                let meta = match MessageMeta::try_from(message) {
                    Ok(meta) => meta,
                    Err(error) => {
                        tracing::error!(?error, "Error parsing jetstream message metadata.");
                        continue;
                    }
                };
                self.has_pending = meta.pending > 0;
                match value {
                    Ok(value) => return Some((value, meta)),
                    Err(error) => {
                        tracing::error!(?error, "Error parsing jetstream message; message ignored.")
                    }
                }
            } else {
                return None;
            }
        }
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
        let subscription = self.nc.subscribe(inbox.clone()).await?;
        self.nc
            .publish_with_reply(
                message.subject(),
                inbox.clone(),
                Bytes::from(serde_json::to_vec(&message)?),
            )
            .await?;

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

    async fn subscribe_jetstream_impl<T: JetStreamable>(
        &self,
        subject: Option<SubscribeSubject<T>>,
    ) -> Result<JetstreamSubscription<T>> {
        // An empty string is equivalent to no subject filter.
        let subject = subject.map(|d| d.subject).unwrap_or_default();
        let _ = self.ensure_jetstream_exists::<T>().await;
        let stream_name = T::stream_name();

        let stream = self.jetstream.get_stream(stream_name).await.to_anyhow()?;
        let has_pending = stream.cached_info().state.messages > 0;
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
            has_pending,
            _ph: PhantomData::default(),
        })
    }

    /// Returns an ORDERED stream of messages published to a nats jetstream, filtered
    /// to a specific subject.
    pub async fn subscribe_jetstream_subject<T: JetStreamable>(
        &self,
        subject: SubscribeSubject<T>,
    ) -> Result<JetstreamSubscription<T>> {
        self.subscribe_jetstream_impl(Some(subject)).await
    }

    /// Returns an ORDERED stream of messages published to a nats jetstream.
    pub async fn subscribe_jetstream<T: JetStreamable>(&self) -> Result<JetstreamSubscription<T>> {
        self.subscribe_jetstream_impl(None).await
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

    pub async fn publish_jetstream<T>(&self, value: &T) -> Result<u64>
    where
        T: TypedMessage<Response = NoReply> + JetStreamable,
    {
        self.ensure_jetstream_exists::<T>().await?;

        let sequence = self
            .jetstream
            .publish(
                value.subject().clone(),
                Bytes::from(serde_json::to_vec(value)?),
            )
            .await
            .to_anyhow()?
            .await
            .to_anyhow()?
            .sequence;

        Ok(sequence)
    }

    /// Publishes a message to jetstream, but only if the subject is empty.
    pub async fn publish_jetstream_if_subject_empty<T>(&self, value: &T) -> Result<Option<u64>>
    where
        T: TypedMessage<Response = NoReply> + JetStreamable,
    {
        self.ensure_jetstream_exists::<T>().await?;

        let publish = Publish::build()
            .payload(Bytes::from(serde_json::to_vec(value)?))
            .expected_last_subject_sequence(0);

        let result = self
            .jetstream
            .send_publish(value.subject().clone(), publish)
            .await
            .to_anyhow()?
            .await
            .to_anyhow();

        match result {
            Ok(result) => Ok(Some(result.sequence)),
            Err(e) => {
                if e.to_string().contains(NATS_WRONG_LAST_SEQUENCE_CODE) {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub async fn request<T>(&self, value: &T) -> Result<T::Response>
    where
        T: TypedMessage,
    {
        let bytes = Bytes::from(serde_json::to_vec(value)?);

        let fut = self.nc.request(value.subject(), bytes.clone());
        let result = fut.await?;
        let value: T::Response = serde_json::from_slice(&result.payload)?;
        Ok(value)
    }

    pub async fn subscribe<T>(&self, subject: SubscribeSubject<T>) -> Result<TypedSubscription<T>>
    where
        T: TypedMessage,
    {
        tracing::info!(stream=%subject.subject, "Subscribing to NATS stream.");
        let subscription = self.nc.subscribe(subject.subject).await?;
        Ok(TypedSubscription::new(subscription, self.nc.clone()))
    }
}
