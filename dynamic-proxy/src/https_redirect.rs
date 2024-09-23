use crate::body::{simple_empty_body, SimpleBody};
use http::{
    header,
    uri::{Authority, Scheme},
    Request, Response, StatusCode, Uri,
};
use hyper::{body::Incoming, service::Service};
use std::{future::ready, pin::Pin, str::FromStr};

#[derive(Debug, Clone)]
pub struct HttpsRedirectService;

impl Service<Request<Incoming>> for HttpsRedirectService {
    type Response = Response<SimpleBody>;
    type Error = http::Error;
    type Future = Pin<Box<std::future::Ready<Result<Response<SimpleBody>, http::Error>>>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        // Get the host header.
        let hostname = request.headers().get(header::HOST).unwrap();
        // Parse the host header into an authority.
        let authority = Authority::from_str(hostname.to_str().unwrap())
            .expect("Valid host is valid authority.");
        // Strip the path.
        let authority =
            Authority::from_str(authority.host()).expect("Valid host is valid authority.");

        let request_uri = request.uri().clone();

        // Set the scheme to HTTPS
        let mut parts = request_uri.into_parts();
        parts.scheme = Some(Scheme::HTTPS);

        // Remove the port from the authority if it exists
        // let authority = parts.authority.map(|d| d.host().to_string()).unwrap();
        // parts.authority = Some(Authority::from_str(authority).unwrap());

        parts.authority = Some(authority);

        // Build the new URI
        let new_uri = Uri::from_parts(parts).unwrap();

        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, new_uri.to_string())
            .body(simple_empty_body());

        Box::pin(ready(response))
    }
}
