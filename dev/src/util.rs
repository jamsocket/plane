use crate::container::build_image;
use anyhow::{anyhow, Result};
use plane_core::messages::agent::{DockerExecutableConfig, SpawnRequest};
use plane_core::messages::scheduler::ScheduleRequest;
use plane_core::state::{StateHandle, WorldState};
use plane_core::types::BackendId;
use plane_core::types::ClusterName;
use plane_core::types::DroneId;
use rand::distributions::Uniform;
use rand::thread_rng;
use rand::Rng;
use std::net::Ipv4Addr;
use std::time::SystemTime;
use std::{
    net::{SocketAddr, SocketAddrV4},
    time::Duration,
};
use tokio::net::TcpSocket;

const POLL_LOOP_SLEEP: u64 = 10;

pub fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Uniform::from('a'..='z'))
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

pub fn spawn_req_with_image(image: String) -> SpawnRequest {
    SpawnRequest {
        cluster: ClusterName::new("plane.test"),
        backend_id: BackendId::new_random(),
        drone_id: DroneId::new_random(),
        metadata: vec![("foo".into(), "bar".into())].into_iter().collect(),
        max_idle_secs: Duration::from_secs(10),
        executable: DockerExecutableConfig {
            image,
            env: vec![("PORT".into(), "8080".into())].into_iter().collect(),
            credentials: None,
            resource_limits: Default::default(),
            pull_policy: Default::default(),
            port: None,
            volume_mounts: vec![],
        },
        bearer_token: None,
    }
}

pub async fn base_spawn_request() -> SpawnRequest {
    spawn_req_with_image(
        build_image("test-images/buildable-test-server")
            .await
            .unwrap(),
    )
}

pub async fn invalid_image_spawn_request() -> SpawnRequest {
    spawn_req_with_image(
        build_image("test-images/invalid-image")
            .await
            .expect("invalid image should build"),
    )
}

pub async fn inactive_image_spawn_request() -> SpawnRequest {
    spawn_req_with_image(
        build_image("test-images/starts-but-no-server")
            .await
            .expect("inactive image should build"),
    )
}

pub fn base_scheduler_request() -> ScheduleRequest {
    ScheduleRequest {
        cluster: ClusterName::new("plane.test"),
        metadata: vec![("foo".into(), "bar".into())].into_iter().collect(),
        backend_id: None,
        max_idle_secs: Duration::from_secs(10),
        executable: DockerExecutableConfig {
            env: vec![("PORT".into(), "8080".into())].into_iter().collect(),
            image: TEST_IMAGE.into(),
            credentials: None,
            resource_limits: Default::default(),
            pull_policy: Default::default(),
            port: None,
            volume_mounts: vec![],
        },
        require_bearer_token: false,
        lock: None,
    }
}

pub async fn wait_for_predicate(
    mut state: StateHandle,
    predicate: impl Fn(&WorldState) -> bool + Send + Sync + 'static,
) {
    tokio::spawn(async move {
        loop {
            let cur_logical_time = {
                let world_state = state.state();
                if predicate(&world_state) {
                    tracing::info!("predicate returned true!");
                    return;
                }
                world_state.logical_time()
            };
            tracing::info!(?cur_logical_time, "waiting for predicate at time!");
            state.wait_for_seq(cur_logical_time + 1).await
        }
    })
    .await
    .unwrap();
}
