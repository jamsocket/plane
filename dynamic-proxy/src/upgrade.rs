use crate::{
    body::{simple_empty_body, SimpleBody},
    proxy::ProxyError,
};
use http::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::copy_bidirectional;

/// Split a request into two requests. The first request has an empty body,
/// and the second request has the original body.
///
/// The first request is forwarded on upstream. The second is used for bidirectional
/// communication after the connection has been upgraded.
pub fn split_request<T>(request: Request<T>) -> (Request<SimpleBody>, Request<T>) {
    let (parts, body) = request.into_parts();

    let request1 = Request::from_parts(parts.clone(), simple_empty_body());
    let request2 = Request::from_parts(parts, body);

    (request1, request2)
}

/// Clone a response, using an empty body.
pub fn split_response<T>(response: Response<T>) -> (Response<SimpleBody>, Response<T>) {
    let (parts, body) = response.into_parts();

    let response1 = Response::from_parts(parts.clone(), simple_empty_body());
    let response2 = Response::from_parts(parts, body);

    (response1, response2)
}

/// Wraps connection state that is needed to upgrade a connection so it can be passed back
/// from the connection handler.
///
/// The receiver should call `.run` to turn this into a future, and then await it.
pub struct UpgradeHandler {
    pub request: Request<SimpleBody>,
    pub response: Response<SimpleBody>,
}

impl UpgradeHandler {
    pub fn new(request: Request<SimpleBody>, response: Response<SimpleBody>) -> Self {
        Self { request, response }
    }

    pub async fn run(self) -> Result<(), ProxyError> {
        let response = hyper::upgrade::on(self.response)
            .await
            .map_err(ProxyError::UpgradeError)?;
        let mut response = TokioIo::new(response);

        let request = hyper::upgrade::on(self.request)
            .await
            .map_err(ProxyError::UpgradeError)?;
        let mut request = TokioIo::new(request);

        copy_bidirectional(&mut request, &mut response)
            .await
            .map_err(ProxyError::IoError)?;

        Ok(())
    }
}
