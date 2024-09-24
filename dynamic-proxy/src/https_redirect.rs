use crate::body::{simple_empty_body, BoxedError, SimpleBody};
use http::{
    header,
    uri::{Authority, Scheme},
    Request, Response, StatusCode, Uri,
};
use hyper::{body::Incoming, service::Service};
use std::{future::ready, pin::Pin, str::FromStr};

/// A hyper service that redirects HTTP requests to HTTPS.
#[derive(Debug, Clone)]
pub struct HttpsRedirectService;

impl HttpsRedirectService {
    fn call_inner(request: Request<Incoming>) -> Result<Response<SimpleBody>, StatusCode> {
        // Get the host header.
        let hostname = request
            .headers()
            .get(header::HOST)
            .ok_or(StatusCode::BAD_REQUEST)?;
        // Parse the host header into an authority.
        let authority =
            Authority::from_str(hostname.to_str().map_err(|_| StatusCode::BAD_REQUEST)?)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
        // Strip the port.
        let authority =
            Authority::from_str(authority.host()).expect("Valid host is always valid authority.");

        let request_uri = request.uri().clone();

        // Set the scheme to HTTPS
        let mut parts = request_uri.into_parts();
        parts.scheme = Some(Scheme::HTTPS);

        parts.authority = Some(authority);

        // Build the new URI
        let new_uri = Uri::from_parts(parts).expect("URI is always valid");

        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, new_uri.to_string())
            .body(simple_empty_body());

        response.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl Service<Request<Incoming>> for HttpsRedirectService {
    type Response = Response<SimpleBody>;
    type Error = BoxedError;
    type Future = Pin<Box<std::future::Ready<Result<Response<SimpleBody>, BoxedError>>>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        let result = Self::call_inner(request);

        let result = match result {
            Ok(response) => response,
            Err(status) => {
                tracing::error!("Error redirecting to HTTPS: {}", status);
                Response::builder()
                    .status(status)
                    .body(simple_empty_body())
                    .unwrap()
            }
        };

        Box::pin(ready(Ok(result)))
    }
}
