use crate::body::{to_simple_body, SimpleBody};
use bytes::Bytes;
use http::{request::Parts, HeaderMap, HeaderName, HeaderValue, Request};
use http_body::Body;
use std::str::FromStr;

/// Represents an HTTP request (from hyper) with helpers for mutating it.
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

    /// Add a header to the request.
    ///
    /// If the header is invalid, it will be ignored and logged.
    pub fn add_header(&mut self, key: &str, value: &str) {
        let Ok(key) = HeaderName::from_str(key) else {
            tracing::error!("Attempted to set invalid header name: {}", key);
            return;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            // Not logging the value, which could be sensitive.
            tracing::error!("Attempted to set invalid header value with key: {}", key);
            return;
        };
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
