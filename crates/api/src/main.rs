use std::net::SocketAddr;

use axum::{routing::post, Router, http::StatusCode, Json};
use clap::{Parser};
use kube::{Client, Api};
use serde::{Deserialize, Serialize};
use spawner_resource::{SessionLivedBackendBuilder, SessionLivedBackend};

const DEFAULT_NAMESPACE: &str = "default";

#[derive(Parser)]
struct Opts {
    #[clap(default_value=DEFAULT_NAMESPACE)]
    namespace: String,

    #[clap(default_value="8080")]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = Opts::parse();

    let app = Router::new()
        .route("/init", post(init_handler));

    let addr = SocketAddr::from(([0, 0, 0, 0], opts.port));
    tracing::debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
    
    Ok(())
}

#[derive(Deserialize)]
struct InitPayload {
    image: String,
}

#[derive(Serialize)]
struct InitResult {
    url: String,
}

async fn init_handler(
    Json(payload): Json<InitPayload>,
) -> Result<Json<InitResult>, StatusCode> {

    let slbe = SessionLivedBackendBuilder::new(&payload.image).build();

    // TODO: use pool?
    let client = Client::try_default().await?;
    let api = Api::<SessionLivedBackend>::namespaced(client, &spec.namespace);
    let slbe = spec.as_slbe();

    let result = api
        .create(
            &PostParams {
                field_manager: Some(SPAWNER_GROUP.to_string()),
                ..PostParams::default()
            },
            &slbe,
        )
        .await?;


    Ok(Json(InitResult {
        url: format!("ok")
    }))
}