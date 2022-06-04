//! Typed wrappers around NATS.
//!
//! These use serde to serialize data to/from JSON over nats into Rust types.

use anyhow::Result;
use nats::asynk::{Connection, Message, Subscription};
use serde::{de::DeserializeOwned, Serialize, Deserialize};
use std::marker::PhantomData;

#[derive(Serialize, Deserialize)]
pub enum NoReply {}

pub struct Subject<M, R> where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    subject: String,
    _ph_m: PhantomData<M>,
    _ph_r: PhantomData<R>,
}

impl<M, R> Subject<M, R> where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    pub fn new(subject: String) -> Subject<M, R> {
        Subject {
            subject,
            _ph_m: PhantomData::default(),
            _ph_r: PhantomData::default(),
        }
    }
}

pub struct SubscribeSubject<M, R> where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    subject: String,
    _ph_m: PhantomData<M>,
    _ph_r: PhantomData<R>,
}

impl<M, R> SubscribeSubject<M, R> where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    pub fn new(subject: String) -> SubscribeSubject<M, R> {
        SubscribeSubject {
            subject,
            _ph_m: PhantomData::default(),
            _ph_r: PhantomData::default(),
        }
    }
}

pub trait Subscribable<M, R>  where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    fn subject(&self) -> &str;
}

impl<M, R> Subscribable<M, R> for &Subject<M, R> where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    fn subject(&self) -> &str {
        &self.subject
    }
}

impl<M, R> Subscribable<M, R> for &SubscribeSubject<M, R> where M: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned {
    fn subject(&self) -> &str {
        &self.subject
    }
}


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
            _ph_t: PhantomData::default()
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
}

impl TypedNats {
    pub async fn connect(nats_url: &str) -> Result<Self> {
        let nc = nats::asynk::connect(nats_url).await?;

        Ok(Self::new(nc))
    }

    pub fn new(nc: Connection) -> Self {
        TypedNats { nc }
    }

    pub async fn publish<T>(&self, subject: &Subject<T, NoReply>, value: &T) -> Result<()>
    where T: Serialize + DeserializeOwned
    {
        self.nc
            .publish(&subject.subject, serde_json::to_vec(value)?)
            .await?;
        Ok(())
    }

    pub async fn request<T, R>(&self, subject: &Subject<T, R>, value: &T) -> Result<R>
    where
        T: Serialize + DeserializeOwned, R: Serialize + DeserializeOwned
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
        R: Serialize + DeserializeOwned
    {
        let subscription = self.nc.subscribe(subject.subject()).await?;
        Ok(TypedSubscription::new(subscription))
    }
}
