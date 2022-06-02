use anyhow::Result;
use nats::asynk::{Connection, Message, Subscription};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;

pub trait TypedMessage: Serialize + DeserializeOwned {
    type Response: Serialize + DeserializeOwned;

    fn subject(&self) -> String;
}

pub struct MessageWithResponseHandle<T>
where
    T: TypedMessage,
{
    pub value: T,
    message: Message,
}

impl<T> MessageWithResponseHandle<T>
where
    T: TypedMessage,
{
    fn new(message: Message) -> Result<Self> {
        Ok(MessageWithResponseHandle {
            value: serde_json::from_slice(&message.data)?,
            message,
        })
    }

    pub async fn respond(&self, response: &T::Response) -> Result<()> {
        Ok(self.message.respond(&serde_json::to_vec(response)?).await?)
    }
}

pub struct TypedSubscription<T>
where
    T: TypedMessage,
{
    subscription: Subscription,
    _ph: PhantomData<T>,
}

impl<T> TypedSubscription<T>
where
    T: TypedMessage,
{
    fn new(subscription: Subscription) -> Self {
        TypedSubscription {
            subscription,
            _ph: PhantomData::default(),
        }
    }

    pub async fn next(&mut self) -> Result<Option<MessageWithResponseHandle<T>>> {
        if let Some(message) = self.subscription.next().await {
            Ok(Some(MessageWithResponseHandle::new(message)?))
        } else {
            Ok(None)
        }
    }
}

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

    pub async fn publish<T>(&self, value: &T) -> Result<()>
    where
        T: TypedMessage<Response = ()>,
    {
        self.nc
            .publish(&value.subject(), serde_json::to_vec(value)?)
            .await?;
        Ok(())
    }

    pub async fn request<T>(&self, value: &T) -> Result<T::Response>
    where
        T: TypedMessage,
    {
        let result = self
            .nc
            .request(&value.subject(), serde_json::to_vec(value)?)
            .await?;

        let value: T::Response = serde_json::from_slice(&result.data)?;
        Ok(value)
    }

    pub async fn subscribe<T>(&self, subject: &str) -> Result<TypedSubscription<T>>
    where
        T: TypedMessage,
    {
        let subscription = self.nc.subscribe(subject).await?;
        Ok(TypedSubscription::new(subscription))
    }
}
