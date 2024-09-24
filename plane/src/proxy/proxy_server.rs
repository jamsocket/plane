use super::{
    connection_monitor::ConnectionMonitorHandle,
    request::{get_and_maybe_remove_bearer_token, subdomain_from_host},
    route_map::RouteMap,
};
use dynamic_proxy::{
    body::{simple_empty_body, SimpleBody},
    hyper::{body::Incoming, header, service::Service, Request, Response, StatusCode, Uri},
    proxy::ProxyClient,
    request::MutableRequest,
};
use std::future::{ready, Future};
use std::{pin::Pin, sync::Arc};

pub struct ProxyStateInner {
    pub route_map: RouteMap,
    pub proxy_client: ProxyClient,
    pub monitor: ConnectionMonitorHandle,
}

#[derive(Clone)]
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
        };

        Self {
            inner: Arc::new(inner),
        }
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

        let Some(bearer_token) = bearer_token else {
            return Box::pin(ready(status_code_to_response(StatusCode::UNAUTHORIZED)));
        };

        request.parts.uri = Uri::from_parts(uri_parts).unwrap();

        let inner = self.inner.clone();

        Box::pin(async move {
            // look up the route info for the bearer token
            let route_info = inner.route_map.lookup(&bearer_token).await;

            let Some(route_info) = route_info else {
                return status_code_to_response(StatusCode::UNAUTHORIZED);
            };

            // Check cluster and subdomain.
            let Some(host) = request
                .parts
                .headers
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
            else {
                return status_code_to_response(StatusCode::BAD_REQUEST);
            };

            let Ok(request_subdomain) = subdomain_from_host(host, &route_info.cluster) else {
                // The host header does not match the expected cluster.
                return status_code_to_response(StatusCode::FORBIDDEN);
            };

            if let Some(subdomain) = route_info.subdomain {
                if request_subdomain != Some(&subdomain) {
                    return status_code_to_response(StatusCode::FORBIDDEN);
                }
            }

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

fn status_code_to_response(
    status_code: StatusCode,
) -> Result<Response<SimpleBody>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Response::builder()
        .status(status_code)
        .body(simple_empty_body())
        .unwrap())
}
