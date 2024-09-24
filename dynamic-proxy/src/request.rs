use crate::body::{to_simple_body, SimpleBody};
use bytes::Bytes;
use http::{
    request::Parts,
    uri::{Authority, Scheme},
    HeaderMap, HeaderName, HeaderValue, Request, Uri,
};
use http_body::Body;
use std::{net::SocketAddr, str::FromStr};

pub struct MutableRequest<T>
where
    T: Body<Data = Bytes> + Send + Sync + 'static,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    pub parts: Parts,
    pub body: T,
}

impl<T> MutableRequest<T>
where
    T: Body<Data = Bytes> + Send + Sync + 'static,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    pub fn from_request(request: Request<T>) -> Self {
        let (parts, body) = request.into_parts();
        Self { parts, body }
    }

    pub fn into_request(self) -> Request<T> {
        Request::from_parts(self.parts, self.body)
    }

    pub fn into_request_with_simple_body(self) -> Request<SimpleBody> {
        Request::from_parts(self.parts, to_simple_body(self.body))
    }

    pub fn set_upstream_address(&mut self, address: SocketAddr) {
        let uri = std::mem::take(&mut self.parts.uri);
        let mut uri_parts = uri.into_parts();
        uri_parts.scheme = Some(Scheme::HTTP);
        uri_parts.authority = Some(
            Authority::try_from(address.to_string())
                .expect("SocketAddr should always be a valid authority."),
        );
        self.parts.uri = Uri::from_parts(uri_parts).expect("URI should always be valid.");
    }

    pub fn add_header(&mut self, key: &str, value: &str) {
        let key = HeaderName::from_str(key).unwrap();
        let value = HeaderValue::from_str(value).unwrap();
        self.parts.headers.append(key, value);
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.parts.headers
    }
}

pub fn should_upgrade<T>(request: &Request<T>) -> bool {
    let Some(conn_header) = request.headers().get("connection") else {
        return false;
    };

    let Ok(conn_header) = conn_header.to_str() else {
        return false;
    };

    conn_header
        .to_lowercase()
        .split(',')
        .any(|s| s.trim() == "upgrade")
}
