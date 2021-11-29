use hyper::Uri;

use crate::ConnectionState;
use std::str::FromStr;

pub async fn get_pod_state(
    pod_name: &str,
    namespace: &str,
    status_port: u16,
) -> anyhow::Result<ConnectionState> {
    // TODO: use monitor port if provided.
    let status_url = Uri::from_str(&format!(
        "http://spawner-{}.{}.svc.cluster.local:{}/status",
        pod_name, namespace, status_port
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
