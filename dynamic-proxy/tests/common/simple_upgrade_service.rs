use bytes::Bytes;
use dynamic_proxy::body::{to_simple_body, SimpleBody};
use http_body_util::{Empty, Full};
use hyper::{
    body::Incoming,
    header::{HeaderValue, UPGRADE},
    service::Service,
    upgrade::Upgraded,
    Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, future::Future, pin::Pin};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct SimpleUpgradeService;

impl Service<Request<Incoming>> for SimpleUpgradeService {
    type Response = Response<SimpleBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<SimpleBody>, Infallible>> + Send>>;

    fn call(&self, mut req: Request<Incoming>) -> Self::Future {
        Box::pin(async move {
            if req.headers().contains_key(UPGRADE) {
                // Handle upgrade
                let mut res = Response::new(to_simple_body(Empty::new()));
                *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                res.headers_mut()
                    .insert(UPGRADE, HeaderValue::from_static("websocket"));

                tokio::task::spawn(async move {
                    if let Ok(upgraded) = hyper::upgrade::on(&mut req).await {
                        if let Err(e) = handle_upgraded_connection(upgraded).await {
                            tracing::error!("Error handling upgraded connection: {}", e);
                        }
                    }
                });

                Ok(res)
            } else {
                // Regular response
                let response = Response::builder()
                    .status(200)
                    .body(to_simple_body(Full::new(Bytes::from("Hello, world!"))))
                    .unwrap();

                Ok(response)
            }
        })
    }
}

async fn handle_upgraded_connection(upgraded: Upgraded) -> std::io::Result<()> {
    let mut upgraded = TokioIo::new(upgraded);

    // echo message back to client
    loop {
        let mut buf = vec![0; 1024];
        let n = upgraded.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        upgraded.write_all(&buf[..n]).await?;
    }

    Ok(())
}
