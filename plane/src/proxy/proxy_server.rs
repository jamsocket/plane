use super::{
    connection_monitor::ConnectionMonitorHandle, request::get_and_maybe_remove_bearer_token,
    route_map::RouteMap,
};
use dynamic_proxy::{
    body::SimpleBody,
    hyper::{body::Incoming, service::Service, Request, Response, Uri},
    proxy::ProxyClient,
    request::MutableRequest,
};
use std::{future::Future, sync::atomic::AtomicBool};
use std::{pin::Pin, sync::Arc};

pub struct ProxyStateInner {
    pub route_map: RouteMap,
    pub proxy_client: ProxyClient,
    pub monitor: ConnectionMonitorHandle,
    pub connected: AtomicBool,
}

pub struct ProxyState {
    pub inner: Arc<ProxyStateInner>,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyState {
    pub fn new() -> Self {
        let inner = ProxyStateInner {
            route_map: RouteMap::new(),
            proxy_client: ProxyClient::new(),
            monitor: ConnectionMonitorHandle::new(),
            connected: AtomicBool::new(false),
        };

        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn set_connected(&self, connected: bool) {
        self.inner
            .connected
            .store(connected, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn connected(&self) -> bool {
        self.inner
            .connected
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Service<Request<Incoming>> for ProxyState {
    type Response = Response<SimpleBody>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = Pin<
        Box<
            dyn Future<
                    Output = Result<Response<SimpleBody>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        let mut request = MutableRequest::from_request(request);

        // extract the bearer token from the request
        let mut uri_parts = request.parts.uri.clone().into_parts();
        let bearer_token = get_and_maybe_remove_bearer_token(&mut uri_parts);

        // TODO
        let bearer_token = bearer_token.unwrap();

        request.parts.uri = Uri::from_parts(uri_parts).unwrap();

        let inner = self.inner.clone();

        Box::pin(async move {
            // look up the route info for the bearer token
            let route_info = inner.route_map.lookup(&bearer_token).await;

            let Some(route_info) = route_info else {
                // TODO
                panic!("Route info not found for bearer token");
            };

            request.set_upstream_address(route_info.address.0);
            let request = request.into_request_with_simple_body();

            let (res, upgrade_handler) = inner.proxy_client.request(request).await.unwrap();

            if let Some(upgrade_handler) = upgrade_handler {
                let monitor = inner.monitor.monitor();
                monitor
                    .lock()
                    .expect("Monitor lock poisoned")
                    .inc_connection(&route_info.backend_id);
                tokio::spawn(async move {
                    upgrade_handler.run().await.unwrap();

                    monitor
                        .lock()
                        .expect("Monitor lock poisoned")
                        .dec_connection(&route_info.backend_id);
                });
            } else {
                inner.monitor.touch_backend(&route_info.backend_id);
            }

            Ok(res)
        })
    }
}
