use hyper::Uri;

use crate::{pod_id::PodId, ConnectionState};
use std::str::FromStr;

pub async fn get_pod_state(
    pod_id: &PodId,
    namespace: &str,
    status_port: u16,
) -> anyhow::Result<ConnectionState> {
    let status_url = Uri::from_str(&format!(
        "http://{}.{}.svc.cluster.local:{}/status", // TODO: don't hardcode
        pod_id.prefixed_name(),
        namespace,
        status_port
    ))
    .expect("Should always be able to construct URL.");
    tracing::info!(%status_url, "Asking container for status.");

    let client = hyper::Client::new();
    let result = client.get(status_url).await?;
    let body = hyper::body::to_bytes(result.into_body()).await?;

    let connection_state: ConnectionState = serde_json::from_slice(&body)?;
    tracing::info!(?connection_state, "Got connection state.");

    Ok(connection_state)
}
