use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use std::env;
use std::process::exit;
use std::time::Duration;
use std::{convert::Infallible, net::SocketAddr};

async fn handle(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    println!("path: {}", req.uri().path());
    if let Some(exit_code) = req.uri().path().strip_prefix("/exit/") {
        let exit_code: i32 = exit_code.parse().expect("Unparseable exit code.");

        exit(exit_code);
    }

    Ok(Response::new("Hello World!".into()))
}

#[tokio::main]
async fn main() {
    if let Some(exit_code) = env::var("EXIT_CODE")
        .ok()
        .map(|c| str::parse::<i32>(&c).expect("Couldn't parse EXIT_CODE."))
    {
        if let Ok(timeout) = env::var("EXIT_TIMEOUT") {
            let timeout = timeout.parse().expect("Couldn't parse EXIT_TIMEOUT.");
            tokio::time::sleep(Duration::from_millis(timeout)).await;
            exit(exit_code)
        }
    }

    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "8080".into())
        .parse()
        .expect("Couldn't parse $PORT as u16.");
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let make_svc = make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(handle)) });

    let server = Server::bind(&addr).serve(make_svc);

    println!("Listening on {}", addr);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
