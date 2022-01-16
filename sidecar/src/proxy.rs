use core::future::Future;
use hyper::client::HttpConnector;
use hyper::http::request::Parts;
use hyper::http::uri::{Authority, InvalidUriParts, Scheme};
use hyper::service::Service;
use hyper::upgrade::Upgraded;
use hyper::{Body, Client, Request, Response, StatusCode, Uri};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::Poll;
use tokio::net::TcpStream;

fn rewrite_uri(upstream: &Authority, uri: &Uri) -> Result<Uri, InvalidUriParts> {
    let mut parts = uri.clone().into_parts();
    parts.authority = Some(upstream.clone());
    parts.scheme = Some(Scheme::HTTP);
    Uri::from_parts(parts)
}

#[derive(Clone)]
pub struct ProxyService {
    pub client: Arc<Client<HttpConnector, Body>>,
    pub upstream: Arc<Authority>,
}

async fn tunnel(mut upgraded: Upgraded, addr: String) -> std::io::Result<()> {
    let mut server = TcpStream::connect(addr).await?;

    // Proxying data
    let (from_client, from_server) =
        tokio::io::copy_bidirectional(&mut upgraded, &mut server).await?;

    // Print message when done
    println!(
        "client wrote {} bytes and received {} bytes",
        from_client, from_server
    );

    Ok(())
}

impl ProxyService {
    pub fn new(upstream: &str) -> anyhow::Result<Self> {
        Ok(ProxyService {
            client: Arc::new(Client::new()),
            upstream: Arc::new(Authority::from_str(upstream)?),
        })
    }

    async fn handle(self, mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
        println!("Request: {:?}", req);

        let mut builder = Request::builder();
        builder = builder.uri(rewrite_uri(&self.upstream, req.uri())?);
        for (key, value) in req.headers() {
            builder = builder.header(key, value);
        }
        builder = builder.method(req.method());
        let upstream_req = builder.body(Body::empty())?;

        let result = self.client.request(upstream_req).await?;

        if result.status() == StatusCode::SWITCHING_PROTOCOLS {
            let mut r2b = Response::builder();
            for (key, value) in result.headers() {
                r2b = r2b.header(key, value);
            }
            r2b = r2b.status(StatusCode::SWITCHING_PROTOCOLS);
            let r2 = r2b.body(Body::empty())?;

            let mut up1 = match hyper::upgrade::on(result).await {
                Ok(upgraded) => {
                    println!("here1 {:?}", upgraded);
                    upgraded
                }
                Err(e) => panic!(),
            };

            tokio::task::spawn(async move {
                match hyper::upgrade::on(&mut req).await {
                    Ok(mut upgraded) => {
                        println!("here2 {:?}", upgraded);

                        let (from_client, from_server) =
                            tokio::io::copy_bidirectional(&mut up1, &mut upgraded)
                                .await
                                .unwrap();

                        println!(
                            "client wrote {} bytes and received {} bytes",
                            from_client, from_server
                        );
                    }
                    Err(e) => eprintln!("upgrade error: {}", e),
                }
            });

            Ok(r2)
        } else {
            println!("result: {:?}", result);
            Ok(result)
        }
    }
}

impl Service<Request<Body>> for ProxyService {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        Box::pin(Self::handle(self.clone(), req))
    }
}
