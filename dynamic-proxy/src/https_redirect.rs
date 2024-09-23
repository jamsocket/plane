use crate::body::{simple_empty_body, SimpleBody};
use http::{header, Request, Response, StatusCode};
use hyper::{body::Incoming, service::Service};
use std::{future::ready, pin::Pin, sync::Arc};

#[derive(Debug, Clone)]
pub struct HttpsRedirectService {
    base_url: Arc<String>,
}

impl HttpsRedirectService {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url: Arc::new(base_url),
        }
    }
}

impl Service<Request<Incoming>> for HttpsRedirectService {
    type Response = Response<SimpleBody>;
    type Error = http::Error;
    type Future = Pin<Box<std::future::Ready<Result<Response<SimpleBody>, http::Error>>>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        let request_uri = request.uri();
        let path_and_query = request_uri.path_and_query();
        let path = path_and_query.map(|pq| pq.as_str()).unwrap_or("/");

        let base_url = self.base_url.as_str();
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, format!("https://{base_url}{path}"))
            .body(simple_empty_body());

        Box::pin(ready(response))
    }
}
