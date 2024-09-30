use dynamic_proxy::{
    https_redirect::HttpsRedirectService,
    server::{HttpsConfig, SimpleHttpServer},
};
use http::{header, StatusCode};
use reqwest::{Response, Url};
use std::net::{IpAddr, SocketAddr};
use tokio::net::TcpListener;

const DOMAIN: &str = "foo.bar.baz";

fn get_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(DOMAIN, SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0))
        .build()
        .unwrap()
}

async fn do_request(url: &str) -> Response {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let https_config = HttpsConfig::http();
    let _server = SimpleHttpServer::new(HttpsRedirectService, listener, https_config);

    // url needs to have port
    let mut url = Url::parse(url).unwrap();
    url.set_port(Some(port)).unwrap();

    // Request to http://foo.bar.baz should redirect to https://foo.bar.baz

    get_client().get(url).send().await.unwrap()
}

#[tokio::test]
async fn test_https_redirect() {
    let response = do_request("http://foo.bar.baz").await;

    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://foo.bar.baz/"
    );
}

#[tokio::test]
async fn test_https_redirect_with_slash_path() {
    let response = do_request("http://foo.bar.baz/").await;

    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://foo.bar.baz/"
    );
}

#[tokio::test]
async fn test_https_redirect_with_path() {
    let response = do_request("http://foo.bar.baz/abc/123").await;

    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://foo.bar.baz/abc/123"
    );
}

#[tokio::test]
async fn test_https_redirect_with_query_params() {
    let response = do_request("http://foo.bar.baz/?a=1&b=2").await;

    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://foo.bar.baz/?a=1&b=2"
    );
}
