use bytes::Bytes;
use dynamic_proxy::body::{to_simple_body, SimpleBody};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, service::Service, Request, Response};
use std::{convert::Infallible, future::Future, pin::Pin};

#[derive(Clone)]
pub struct HelloWorldService;

impl Service<Request<Incoming>> for HelloWorldService {
    type Response = Response<SimpleBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<SimpleBody>, Infallible>> + Send>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        Box::pin(async {
            let _ = request.collect().await.unwrap().to_bytes();

            let response = Response::builder()
                .status(200)
                .body(to_simple_body(Full::new(Bytes::from("Hello, world!"))))
                .unwrap();

            Ok(response)
        })
    }
}
