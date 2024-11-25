use plane_common::types::backend_state::BackendError;
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const WAIT_TIMEOUT_MS: u64 = 100;
const REQUEST_TIMEOUT_MS: u64 = 1_000;
const STARTUP_TIMEOUT_SECS: u64 = 300; // 5 min timeout
const HEAD_REQUEST: &[u8] = b"HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n";

/// Waits until an HTTP request to the given socket address succeeds or times out after 5 minutes.
pub async fn wait_for_backend(address: SocketAddr) -> Result<(), BackendError> {
    let start = Instant::now();
    let timeout_duration = Duration::from_secs(STARTUP_TIMEOUT_SECS);
    loop {
        if start.elapsed() > timeout_duration {
            return Err(BackendError::StartupTimeout);
        }

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

        if result.is_err() {
            // error
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        }

        if &buffer == b"HTTP/" {
            // success
            break Ok(());
        } else {
            // not ready yet
            tokio::time::sleep(Duration::from_millis(WAIT_TIMEOUT_MS)).await;
            continue;
        }
    }
}
