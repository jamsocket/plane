use crate::{
    protocol::{RouteInfo, RouteInfoRequest, RouteInfoResponse},
    types::BearerToken,
};
use lru::LruCache;
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{Mutex, RwLock},
};
use tokio::sync::broadcast::Sender;
use valuable::Valuable;

const CACHE_SIZE: usize = 1_000;

type RequestSender = Box<dyn Fn(RouteInfoRequest) + Send + Sync + 'static>;

pub struct RouteMap {
    pub routes: Mutex<LruCache<BearerToken, Option<RouteInfo>>>,
    pub request_sender: RwLock<Option<RequestSender>>,
    pub listeners: Mutex<HashMap<BearerToken, Sender<()>>>,
}

impl Default for RouteMap {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteMap {
    pub fn new() -> Self {
        Self {
            routes: Mutex::new(LruCache::new(
                NonZeroUsize::new(CACHE_SIZE).expect("Always valid conversion from constant."),
            )),
            request_sender: RwLock::new(None),
            listeners: Mutex::default(),
        }
    }

    pub fn set_sender<F>(&self, sender: F)
    where
        F: Fn(RouteInfoRequest) + Send + Sync + 'static,
    {
        *self
            .request_sender
            .write()
            .expect("Request sender was poisoned.") = Some(Box::new(sender));
    }

    pub async fn lookup(&self, token: &BearerToken) -> Option<RouteInfo> {
        {
            let mut lock = self.routes.lock().expect("Routes lock was poisoned.");
            if let Some(route_info) = lock.get(token) {
                return route_info.clone();
            }
        }

        let mut receiver = {
            let mut listener_lock = self.listeners.lock().expect("Listeners lock was poisoned.");
            let sender = listener_lock.entry(token.clone()).or_insert_with(|| {
                let (sender, _) = tokio::sync::broadcast::channel(1);
                sender
            });
            sender.subscribe()
        };

        let message = RouteInfoRequest {
            token: token.clone(),
        };

        {
            let maybe_request_sender = self
                .request_sender
                .read()
                .expect("Request sender was poisoned.");

            let request_sender = match maybe_request_sender.as_ref() {
                Some(request_sender) => request_sender,
                None => return None,
            };

            (request_sender)(message);
        }

        receiver
            .recv()
            .await
            .expect("Always able to receive from channel.");
        self.routes
            .lock()
            .expect("Routes lock was poisoned.")
            .get(token)
            .and_then(|x| x.clone())
    }

    fn insert(&self, token: BearerToken, route_info: Option<RouteInfo>) {
        tracing::info!(
            token = token.as_value(),
            ?route_info,
            "Inserting route info"
        );
        self.routes
            .lock()
            .expect("Routes lock was poisoned.")
            .push(token.clone(), route_info);
        let listener_lock = self.listeners.lock().expect("Listeners lock was poisoned.");
        if let Some(listener_lock) = listener_lock.get(&token) {
            if let Err(err) = listener_lock.send(()) {
                tracing::error!(?err, "Error sending to listener.");
            }
        };
    }

    pub fn receive(&self, response: RouteInfoResponse) {
        self.insert(response.token, response.route_info);
    }
}
