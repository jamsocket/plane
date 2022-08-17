use std::{time::Duration, net::{IpAddr, SocketAddr, SocketAddrV4, Ipv4Addr}};
use anyhow::{anyhow, Result};
use rand::{distributions::Alphanumeric, Rng};
use tokio::net::TcpSocket;
use std::time::SystemTime;

const POLL_LOOP_SLEEP: u64 = 10;

pub fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

pub async fn wait_for_port(addr: SocketAddrV4, timeout_ms: u128) -> Result<()> {
    let initial_time = SystemTime::now();
    
    loop {
        let socket = TcpSocket::new_v4()?;
        let result = socket.connect(SocketAddr::V4(addr)).await;

        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                if SystemTime::now()
                    .duration_since(initial_time)
                    .unwrap()
                    .as_millis()
                    > timeout_ms
                {
                    return Err(anyhow!(
                        "Failed to access {:?} after {}ms. Last error was {:?}",
                        addr,
                        timeout_ms,
                        e
                    ));
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(POLL_LOOP_SLEEP)).await;
    }
}

pub async fn wait_for_url(url: &str, timeout_ms: u128) -> Result<()> {
    let initial_time = SystemTime::now();
    let client = reqwest::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()?;
    loop {
        let result = client.get(url.clone()).timeout(Duration::from_secs(1)).send().await;

        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                if SystemTime::now()
                    .duration_since(initial_time)
                    .unwrap()
                    .as_millis()
                    > timeout_ms
                {
                    return Err(anyhow!(
                        "Failed to load URL {} after {}ms. Last error was {:?}",
                        url,
                        timeout_ms,
                        e
                    ));
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(POLL_LOOP_SLEEP)).await;
    }
}
