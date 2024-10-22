use bytes::Bytes;
use plane_dynamic_proxy::body::{to_simple_body, SimpleBody};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, service::Service, Request, Response};
use std::{convert::Infallible, future::Future, pin::Pin};

/// A service that returns a greeting with the X-Forwarded-For and X-Forwarded-Proto headers.
#[derive(Clone)]
pub struct HelloWorldService;

impl Service<Request<Incoming>> for HelloWorldService {
    type Response = Response<SimpleBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<SimpleBody>, Infallible>> + Send>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        Box::pin(async {
            let x_forwarded_for = request
                .headers()
                .get("x-forwarded-for")
                .map(|h| h.to_str().unwrap().to_string())
                .unwrap_or_default();
            let x_forwarded_proto = request
                .headers()
                .get("x-forwarded-proto")
                .map(|h| h.to_str().unwrap().to_string())
                .unwrap_or_default();

            let _ = request.collect().await.unwrap().to_bytes();

            let body = format!(
                "Hello, world! X-Forwarded-For: {}, X-Forwarded-Proto: {}",
                x_forwarded_for, x_forwarded_proto
            );

            let response = Response::builder()
                .status(200)
                .body(to_simple_body(Full::new(Bytes::from(
                    body.as_bytes().to_vec(),
                ))))
                .unwrap();

            Ok(response)
        })
    }
}
