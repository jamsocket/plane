use super::{
    connection_monitor::ConnectionMonitorHandle,
    request::{get_and_maybe_remove_bearer_token, subdomain_from_host},
    route_map::RouteMap,
};
use crate::{names::Name, protocol::RouteInfo, SERVER_NAME};
use bytes::Bytes;
use dynamic_proxy::{
    body::{simple_empty_body, SimpleBody},
    hyper::{
        body::{Body, Incoming},
        header::{self, HeaderValue},
        service::Service,
        Request, Response, StatusCode, Uri,
    },
    proxy::ProxyClient,
    request::MutableRequest,
};
use std::{
    future::{ready, Future},
    sync::atomic::{AtomicBool, Ordering},
};
use std::{pin::Pin, sync::Arc};

pub struct ProxyStateInner {
    pub route_map: RouteMap,
    pub proxy_client: ProxyClient,
    pub monitor: ConnectionMonitorHandle,
    pub connected: AtomicBool,

    /// If set, the "root" path (/) will redirect to this URL.
    pub root_redirect_url: Option<String>,
}

#[derive(Clone)]
pub struct ProxyState {
    pub inner: Arc<ProxyStateInner>,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ProxyState {
    pub fn new(root_redirect_url: Option<String>) -> Self {
        let inner = ProxyStateInner {
            route_map: RouteMap::new(),
            proxy_client: ProxyClient::new(),
            monitor: ConnectionMonitorHandle::new(),
            connected: AtomicBool::new(false),
            root_redirect_url,
        };

        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn set_ready(&self, ready: bool) {
        self.inner.connected.store(ready, Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        self.inner.connected.load(Ordering::Relaxed)
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
        // Handle "/ready"
        if request.uri().path() == "/ready" {
            if self.is_ready() {
                return Box::pin(ready(status_code_to_response(StatusCode::OK)));
            } else {
                return Box::pin(ready(status_code_to_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                )));
            }
        }

        if request.uri().path() == "/" {
            if let Some(root_redirect_url) = &self.inner.root_redirect_url {
                let mut response = Response::builder()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, root_redirect_url)
                    .body(simple_empty_body())
                    .expect("Failed to build response");

                apply_general_headers(&mut response);

                return Box::pin(ready(Ok(response)));
            } else {
                return Box::pin(ready(status_code_to_response(StatusCode::BAD_REQUEST)));
            }
        }

        let mut request = MutableRequest::from_request(request);

        // extract the bearer token from the request
        let mut uri_parts = request.parts.uri.clone().into_parts();
        let original_path = request.parts.uri.path().to_string();
        let bearer_token = get_and_maybe_remove_bearer_token(&mut uri_parts);

        let Some(bearer_token) = bearer_token else {
            // This should have already been handled by the root redirect above.
            return Box::pin(ready(status_code_to_response(StatusCode::BAD_REQUEST)));
        };

        let Ok(uri) = Uri::from_parts(uri_parts) else {
            return Box::pin(ready(status_code_to_response(StatusCode::BAD_REQUEST)));
        };
        request.parts.uri = uri;

        let inner = self.inner.clone();

        Box::pin(async move {
            // look up the route info for the bearer token
            let route_info = inner.route_map.lookup(&bearer_token).await;

            let Some(route_info) = route_info else {
                return status_code_to_response(StatusCode::GONE);
            };

            if let Err(status_code) = prepare_request(&mut request, &route_info, &original_path) {
                return status_code_to_response(status_code);
            }

            let request = request.into_request_with_simple_body();

            let result = inner.proxy_client.request(request).await;

            let (mut res, upgrade_handler) = match result {
                Ok((res, upgrade_handler)) => (res, upgrade_handler),
                Err(e) => {
                    tracing::error!("Error proxying request: {}", e);
                    return status_code_to_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            if let Some(upgrade_handler) = upgrade_handler {
                let monitor = inner.monitor.monitor();
                monitor
                    .lock()
                    .expect("Monitor lock poisoned")
                    .inc_connection(&route_info.backend_id);
                let backend_id = route_info.backend_id.clone();
                tokio::spawn(async move {
                    if let Err(err) = upgrade_handler.run().await {
                        tracing::error!("Error running upgrade handler: {}", err);
                    };

                    monitor
                        .lock()
                        .expect("Monitor lock poisoned")
                        .dec_connection(&backend_id);
                });
            } else {
                inner.monitor.touch_backend(&route_info.backend_id);
            }

            apply_general_headers(&mut res);
            res.headers_mut().insert(
                "x-plane-backend-id",
                HeaderValue::from_str(&route_info.backend_id.to_string())
                    .expect("Backend ID is always a valid header value"),
            );

            Ok(res)
        })
    }
}

fn prepare_request<T>(
    request: &mut MutableRequest<T>,
    route_info: &RouteInfo,
    original_path: &str,
) -> Result<(), StatusCode>
where
    T: Body<Data = Bytes> + Send + Sync,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    // Check cluster and subdomain.
    let Some(host) = request
        .parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
    else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let Ok(request_subdomain) = subdomain_from_host(host, &route_info.cluster) else {
        // The host header does not match the expected cluster.
        return Err(StatusCode::FORBIDDEN);
    };

    if let Some(subdomain) = &route_info.subdomain {
        if request_subdomain != Some(subdomain) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    request.set_upstream_address(route_info.address.0);

    // Remove x-verified-* headers from inbound request.
    {
        let headers = request.headers_mut();
        let mut headers_to_remove = Vec::new();
        headers.iter_mut().for_each(|(name, _)| {
            if name.as_str().starts_with("x-verified-") {
                headers_to_remove.push(name.clone());
            }
        });

        for header in headers_to_remove {
            headers.remove(&header);
        }
    }

    // Set special Plane headers.
    if let Some(username) = &route_info.user {
        request.add_header("x-verified-username", username);
    }

    if let Some(user_data) = &route_info.user_data {
        let user_data_str = serde_json::to_string(user_data).unwrap_or_default();
        request.add_header("x-verified-user-data", &user_data_str);
    }

    request.add_header("x-verified-path", original_path);
    request.add_header("x-verified-backend", route_info.backend_id.as_str());
    request.add_header("x-verified-secret", route_info.secret_token.as_str());

    Ok(())
}

fn status_code_to_response(
    status_code: StatusCode,
) -> Result<Response<SimpleBody>, Box<dyn std::error::Error + Send + Sync>> {
    let mut response = Response::builder()
        .status(status_code)
        .body(simple_empty_body())
        .expect("Failed to build response");

    apply_general_headers(&mut response);

    Ok(response)
}

/// Mutates a request to add static headers present on all responses
/// (error or valid).
fn apply_general_headers(response: &mut Response<SimpleBody>) {
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("*"),
    );
    headers.insert(header::SERVER, HeaderValue::from_static(SERVER_NAME));
}
