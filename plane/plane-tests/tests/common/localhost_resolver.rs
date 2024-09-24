use hyper::client::connect::dns::Name;
use reqwest::dns::{Resolve, Resolving};
use std::{future::ready, net::SocketAddr, sync::Arc};

/// A reqwest-compatible DNS resolver that resolves all requests to localhost.
struct LocalhostResolver;

impl Resolve for LocalhostResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        let addrs = vec![SocketAddr::from(([127, 0, 0, 1], 0))];
        let addrs: Box<dyn Iterator<Item = SocketAddr> + Send> = Box::new(addrs.into_iter());
        Box::pin(ready(Ok(addrs)))
    }
}

#[allow(unused)]
pub fn localhost_client() -> reqwest::Client {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(LocalhostResolver))
        .build()
        .unwrap()
}
