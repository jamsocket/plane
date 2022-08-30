use anyhow::{anyhow, Result};
use dis_spawner::messages::agent::SpawnRequest;
use dis_spawner::messages::scheduler::{ScheduleRequest, ClusterId};
use dis_spawner::types::BackendId;
use dis_spawner::types::DroneId;
use rand::distributions::Alphanumeric;
use rand::thread_rng;
use rand::Rng;
use std::net::Ipv4Addr;
use std::time::SystemTime;
use std::{
    net::{SocketAddr, SocketAddrV4},
    time::Duration,
};
use tokio::net::TcpSocket;

use crate::test_name;

const POLL_LOOP_SLEEP: u64 = 10;

pub fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

pub fn random_prefix(suffix: &str) -> String {
    let prefix: String = random_string(6);
    format!("{}-{}", prefix, suffix)
}

pub fn random_loopback_ip() -> Ipv4Addr {
    let mut rng = thread_rng();
    let v1 = rng.gen_range(1..254);
    let v2 = rng.gen_range(1..254);
    let v3 = rng.gen_range(1..254);

    Ipv4Addr::new(127, v1, v2, v3)
}

pub fn random_backend_id(hint: &str) -> BackendId {
    BackendId::new(random_prefix(hint))
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
        let result = client.get(url).timeout(Duration::from_secs(1)).send().await;

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

const TEST_IMAGE: &str = "ghcr.io/drifting-in-space/test-image:latest";

pub fn base_spawn_request() -> SpawnRequest {
    let backend_id = random_backend_id(&test_name());
    SpawnRequest {
        drone_id: DroneId::new(0),
        image: TEST_IMAGE.into(),
        backend_id: backend_id.clone(),
        max_idle_secs: Duration::from_secs(10),
        env: vec![("PORT".into(), "8080".into())].into_iter().collect(),
        metadata: vec![("foo".into(), "bar".into())].into_iter().collect(),
        credentials: None,
    }
}

pub fn base_scheduler_request() -> ScheduleRequest {
    let backend_id = random_backend_id(&test_name());
    ScheduleRequest {
        cluster: ClusterId::new("spawner.test"),
        image: TEST_IMAGE.into(),
        backend_id: backend_id.clone(),
        max_idle_secs: Duration::from_secs(10),
        env: vec![("PORT".into(), "8080".into())].into_iter().collect(),
        metadata: vec![("foo".into(), "bar".into())].into_iter().collect(),
        credentials: None,
    }
}
