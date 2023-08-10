//! Typed wrappers around NATS.
//!
//! These use serde to serialize data to/from JSON over nats into Rust types.

use crate::logging::LogError;
use crate::messages::initialize_jetstreams;
use anyhow::{anyhow, Context, Result};
use async_nats::jetstream;
use async_nats::jetstream::consumer::push::Messages;
use async_nats::jetstream::consumer::DeliverPolicy;
use async_nats::jetstream::context::Publish;
use async_nats::jetstream::stream::Config;
use async_nats::{Client, Message, Subscriber};
use bytes::Bytes;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::task::Poll;
use std::time::Duration;
use time::OffsetDateTime;
use tokio_stream::{Stream, StreamExt};

/// This code is returned by NATS when an expected last sequence number is violated.
const NATS_WRONG_LAST_SEQUENCE_CODE: &str = "10071";

/// How long NATS should keep consumer state before forgetting it.
/// This is effectively how long a subscriber can sustain a NATS outage. Even though
/// we expect NATS outages to be short, there's little harm in making this too long;
/// consumer state is minimal and we make efficient use of consumers.
const INACTIVE_THRESHOLD_SECONDS: u64 = 60 * 60 * 12; // 12 hours

/// Unconstructable type, used as a [TypedMessage::Response] to indicate that
/// no response is allowed.
#[derive(Serialize, Deserialize)]
pub enum NoReply {}

#[derive(Debug)]
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
            _ph_m: PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
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
    let expected_subject = value.subject();
    if expected_subject != message.subject {
        if expected_subject.starts_with("state.cluster.")
            && expected_subject.ends_with(".assignment")
        {
            // Temporarily ignore due to subject renaming.
        } else {
            return Err(anyhow!(
                "Message subject ({}) does not match expected subject ({})",
                message.subject,
                value.subject()
            ));
        }
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
            _ph_t: PhantomData,
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

impl<T> Stream for TypedSubscription<T>
where
    T: TypedMessage + std::marker::Unpin,
{
    type Item = MessageWithResponseHandle<T>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<MessageWithResponseHandle<T>>> {
        let mut fut1 = Box::pin(Self::next(&mut self));
        fut1.as_mut().poll(cx)
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
    pub jetstream: jetstream::Context,
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

    has_pending: bool,

    _ph: PhantomData<T>,
}

impl<T: TypedMessage> JetstreamSubscription<T> {
    pub fn has_pending(&self) -> bool {
        self.has_pending
    }

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
                let value = match value {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::error!(
                            ?error,
                            ?message,
                            "Error parsing jetstream message; message ignored."
                        );
                        continue;
                    }
                };

                let meta = match MessageMeta::try_from(message) {
                    Ok(meta) => meta,
                    Err(error) => {
                        tracing::error!(?error, "Error parsing jetstream message metadata.");
                        continue;
                    }
                };

                self.has_pending = meta.pending > 0;

                return Some((value, meta));
            } else {
                return None;
            }
        }
    }
}

impl TypedNats {
    pub async fn new(nc: Client) -> Result<Self> {
        let jetstream = async_nats::jetstream::new(nc.clone());

        Ok(TypedNats { nc, jetstream })
    }

    pub async fn initialize_jetstreams(&self) -> Result<()> {
        initialize_jetstreams(&self.jetstream).await
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
            _ph: PhantomData,
        })
    }

    async fn subscribe_jetstream_impl<T: JetStreamable>(
        &self,
        subject: Option<SubscribeSubject<T>>,
    ) -> Result<JetstreamSubscription<T>> {
        // An empty string is equivalent to no subject filter.
        let subject = subject.map(|d| d.subject).unwrap_or_default();
        let stream_name = T::stream_name();

        let stream = self.jetstream.get_stream(stream_name).await.to_anyhow()?;
        let deliver_subject = self.nc.new_inbox();

        let consumer = stream
            .create_consumer(async_nats::jetstream::consumer::push::Config {
                deliver_policy: DeliverPolicy::All,
                filter_subject: subject,
                deliver_subject,
                inactive_threshold: Duration::from_secs(INACTIVE_THRESHOLD_SECONDS),
                max_ack_pending: 1, // NOTE: If you remove this or change the value,
                // the resultant stream is no longer guaranteed to be in order, and call sites
                // that rely on ordered messages will break, nondeterministically
                ..Default::default()
            })
            .await
            .to_anyhow()?;

        let stream: Messages = consumer.messages().await.to_anyhow()?;
        let has_pending = consumer.cached_info().num_pending > 0;

        Ok(JetstreamSubscription {
            stream,
            has_pending,
            _ph: PhantomData,
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

    pub async fn request_with_timeout<T>(&self, value: &T, timeout: Duration) -> Result<T::Response>
    where
        T: TypedMessage,
    {
        let bytes = Bytes::from(serde_json::to_vec(value)?);

        let request = async_nats::Request::new()
            .payload(bytes)
            .timeout(Some(timeout));
        let fut = self.nc.send_request(value.subject(), request);
        let result = fut.await.unwrap();
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
