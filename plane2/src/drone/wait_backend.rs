use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const WAIT_TIMEOUT_MS: u64 = 100;
const REQUEST_TIMEOUT_MS: u64 = 1_000;
const HEAD_REQUEST: &[u8] = b"HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n";

/// Waits until an HTTP request to the given socket address succeeds.
/// Retries indefinitely.
pub async fn wait_for_backend(address: SocketAddr) {
    loop {
        let Ok(mut conn) = TcpStream::connect(address).await else {
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        };

        if conn.write_all(HEAD_REQUEST).await.is_err() {
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        }

        let mut buffer = [0; 5]; // We expect the response to start with b"HTTP/"
        let Ok(result) = tokio::time::timeout(
            Duration::from_millis(REQUEST_TIMEOUT_MS),
            conn.read(&mut buffer),
        )
        .await
        else {
            // timed out
            tracing::warn!("Timed out reading from socket.");
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        };

        if let Err(e) = result {
            // error
            tracing::warn!(%e, "Error reading from socket.");
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        }

        if &buffer == b"HTTP/" {
            // success
            break;
        } else {
            // not ready yet
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        }
    }
}
