use anyhow::{anyhow, Result};
use integration_test::integration_test;
use plane_core::{
    messages::{
        agent::{BackendState, DroneState, SpawnRequest, TerminationRequest, BackendStatsMessage},
        scheduler::DrainDrone,
    },
    nats::TypedNats,
    types::{BackendId, ClusterName, DroneId},
    views::{replica::SystemViewReplica, DroneView},
    NeverResult,
};
use plane_dev::{
    resources::{nats::Nats, server::Server},
    scratch_dir,
    timeout::{self, expect_to_stay_alive, timeout, LivenessGuard},
    util::{base_spawn_request, random_loopback_ip},
};
use plane_drone::config::DockerConfig;
use plane_drone::{agent::AgentOptions, database::DroneDatabase, ip::IpSource};
use std::net::IpAddr;
use std::time::Duration;
use tokio::time::Instant;

const CLUSTER_DOMAIN: &str = "plane.test";

struct Agent {
    #[allow(unused)]
    agent_guard: LivenessGuard<NeverResult>,
    pub ip: IpAddr,
    pub db: DroneDatabase,
}

impl Agent {
    pub async fn new(nats: &Nats, drone_id: &DroneId) -> Result<Agent> {
        let ip = random_loopback_ip();
        let db = DroneDatabase::new(&scratch_dir("agent").join("drone.db")).await?;

        let agent_opts = AgentOptions {
            db: db.clone(),
            drone_id: drone_id.clone(),
            nats: nats.connection().await?,
            cluster_domain: ClusterName::new(CLUSTER_DOMAIN),
            ip: IpSource::Literal(ip),
            docker_options: DockerConfig::default(),
        };

        let agent_guard = expect_to_stay_alive(plane_drone::agent::run_agent(agent_opts));

        Ok(Agent {
            agent_guard,
            ip,
            db,
        })
    }
}

struct MockController {
    view: SystemViewReplica,
    nats: TypedNats,
}

impl MockController {
    pub async fn new(nats: &Nats) -> Result<MockController> {
        let nats = nats.connection().await?;
        let view = SystemViewReplica::new(nats.clone()).await?;
        Ok(MockController { view, nats })
    }

    pub fn drone(&self, drone_id: &DroneId) -> Result<DroneView> {
        self.view
            .view()
            .cluster(&ClusterName::new(CLUSTER_DOMAIN))
            .ok_or_else(|| anyhow!("Cluster not found"))?
            .drone(drone_id)
            .ok_or_else(|| anyhow!("Drone {} not found in cluster", drone_id))
            .map(|drone| drone.clone())
    }

    /// Waits for the given drone to be in the given state. Returns an error if the drone is not in
    /// the given state after the given timeout.
    pub async fn wait_for_drone_state(
        &self,
        drone_id: &DroneId,
        state: DroneState,
        timeout_seconds: u64,
    ) -> Result<()> {
        let start = Instant::now();
        loop {
            let result = self.drone(drone_id);
            if let Ok(drone) = &result {
                if drone.state() == Some(state) {
                    return Ok(());
                }
            }

            if start.elapsed() > Duration::from_secs(timeout_seconds) {
                return Err(anyhow!(
                    "Drone {} did not reach state {:?} after {} seconds. Last result: {:?}",
                    drone_id,
                    state,
                    timeout_seconds,
                    result
                ));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Waits for the given backend to be in the given state. Returns an error if the backend is not
    /// in the given state after the given timeout.
    pub async fn wait_for_backend_state(
        &self,
        drone_id: &DroneId,
        backend_id: &BackendId,
        state: BackendState,
        timeout_seconds: u64,
    ) -> Result<()> {
        let start = Instant::now();
        loop {
            let result = self.drone(drone_id);
            if let Ok(drone) = &result {
                if let Some(backend_state) = drone.backends.get(backend_id) {
                    if *backend_state == state {
                        return Ok(());
                    }
                }
            }

            if start.elapsed() > Duration::from_secs(timeout_seconds) {
                return Err(anyhow!(
                    "Backend {} did not reach state {:?} after {} seconds. Last result: {:?}",
                    backend_id,
                    state,
                    timeout_seconds,
                    result
                ));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // pub async fn wait_for_backend_removed(
    //     &self,
    //     drone_id: &DroneId,
    //     backend_id: &BackendId,
    //     timeout_seconds: u64,
    // ) -> Result<()> {
    //     let start = Instant::now();
    //     loop {
    //         let result = self.drone(drone_id);
    //         if let Ok(drone) = &result {
    //             if drone.backends.get(backend_id).is_none() {
    //                 return Ok(());
    //             }
    //         }

    //         if start.elapsed() > Duration::from_secs(timeout_seconds) {
    //             return Err(anyhow!(
    //                 "Backend {} was not removed after {} seconds. Last result: {:?}",
    //                 backend_id,
    //                 timeout_seconds,
    //                 result
    //             ));
    //         }

    //         tokio::time::sleep(Duration::from_millis(100)).await;
    //     }
    // }

    pub async fn spawn_backend(&self, request: &SpawnRequest) -> Result<()> {
        let result = timeout(
            10_000,
            "Spawn request acknowledged by agent.",
            self.nats.request(request),
        )
        .await??;

        assert!(result, "Spawn request should result in response of _true_.");
        Ok(())
    }

    pub async fn terminate_backend(&self, request: &TerminationRequest) -> Result<()> {
        timeout(10_000, "Termination!", self.nats.request(request)).await??;

        Ok(())
    }
}

#[integration_test]
async fn drone_status() {
    let nats = Nats::new().await.unwrap();
    let controller_mock = MockController::new(&nats).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id).await.unwrap();

    controller_mock
        .wait_for_drone_state(&drone_id, DroneState::Ready, 10)
        .await
        .unwrap();
}

#[integration_test]
async fn drone_sends_draining_status() {
    let nats = Nats::new().await.unwrap();
    let nats_connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(&nats).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id).await.unwrap();

    controller_mock
        .wait_for_drone_state(&drone_id, DroneState::Ready, 10)
        .await
        .unwrap();

    timeout(
        1_000,
        "Did not receive DrainDrone response",
        nats_connection.request(&DrainDrone {
            cluster: ClusterName::new(CLUSTER_DOMAIN),
            drone: drone_id.clone(),
            drain: true,
        }),
    )
    .await
    .unwrap()
    .unwrap();

    controller_mock
        .wait_for_drone_state(&drone_id, DroneState::Draining, 10)
        .await
        .unwrap();
}

#[integration_test]
async fn spawn_with_agent() {
    let nats = Nats::new().await.unwrap();
    let controller_mock = MockController::new(&nats).await.unwrap();
    let drone_id = DroneId::new_random();
    let agent = Agent::new(&nats, &drone_id).await.unwrap();

    controller_mock
        .wait_for_drone_state(&drone_id, DroneState::Ready, 10)
        .await
        .unwrap();

    let mut request = base_spawn_request();
    request.drone_id = drone_id.clone();

    controller_mock.spawn_backend(&request).await.unwrap();

    controller_mock
        .wait_for_backend_state(&drone_id, &request.backend_id, BackendState::Ready, 10)
        .await
        .unwrap();

    let proxy_route = agent
        .db
        .get_proxy_route(request.backend_id.id())
        .await
        .unwrap()
        .expect("Expected proxy route.");
    let result = reqwest::get(format!("http://{}/", proxy_route))
        .await
        .unwrap();
    assert_eq!("Hello World!", result.text().await.unwrap());

    controller_mock
        .wait_for_backend_state(&drone_id, &request.backend_id, BackendState::Swept, 10)
        .await
        .unwrap();

    // Route is invalidated after sweeping.
    assert!(
        agent
            .db
            .get_proxy_route(request.backend_id.id())
            .await
            .unwrap()
            .is_none(),
        "Expected proxy route to be swept after backend is stopped."
    );
}

#[integration_test]
async fn stats_are_acquired() {
    let nats = Nats::new().await.unwrap();
    let connection = nats.connection().await.unwrap();
    let controller_mock = MockController::new(&nats).await.unwrap();
    let drone_id = DroneId::new_random();
    let _agent = Agent::new(&nats, &drone_id).await.unwrap();
    controller_mock.wait_for_drone_state(&drone_id, DroneState::Ready, 10).await.unwrap();

    let mut request = base_spawn_request();
    request.drone_id = drone_id;
    // Ensure long enough life to report stats.
    request.max_idle_secs = Duration::from_secs(30);

    controller_mock.spawn_backend(&request).await.unwrap();
    controller_mock.wait_for_backend_state(&request.drone_id, &request.backend_id, BackendState::Ready, 60).await.unwrap();

    let mut stats_subscription = connection
        .subscribe(BackendStatsMessage::subscribe_subject(&request.backend_id))
        .await
        .unwrap();

    let stat = timeout(
        30_000,
        "Waiting for stats message.",
        stats_subscription.next(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(stat.value.cpu_use_percent >= 0.);
    assert!(stat.value.mem_use_percent >= 0.);

    controller_mock.wait_for_backend_state(&request.drone_id, &request.backend_id, BackendState::Swept, 30).await.unwrap();
}

#[integration_test]
async fn use_ip_lookup_api() {
    let server = Server::new(|_| async { "123.11.22.33".to_string() })
        .await
        .unwrap();

    let provider = IpSource::Api { api: server.url() };

    let result = provider.get_ip().await.unwrap();
    assert_eq!("123.11.22.33".parse::<IpAddr>().unwrap(), result);
}

// #[integration_test]
// async fn handle_error_during_start() {
//     let nats = Nats::new().await.unwrap();
//     let connection = nats.connection().await.unwrap();
//     let mut controller_mock = MockController::new(connection.clone()).await.unwrap();
//     let drone_id = DroneId::new_random();
//     let agent = Agent::new(&nats, &drone_id).await.unwrap();
//     controller_mock
//         .expect_drone_status(&drone_id, agent.ip)
//         .await
//         .unwrap();

//     controller_mock
//         .expect_backend_status(&drone_id, &ClusterName::new("plane.test"), true, 0)
//         .await
//         .unwrap();

//     let mut request = base_spawn_request();
//     request.drone_id = drone_id;
//     // Exit with error code 1 after 100ms.
//     request
//         .executable
//         .env
//         .insert("EXIT_CODE".into(), "1".into());
//     request
//         .executable
//         .env
//         .insert("EXIT_TIMEOUT".into(), "100".into());

//     let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
//         .await
//         .unwrap();
//     controller_mock.spawn_backend(&request).await.unwrap();

//     state_subscription
//         .expect_backend_status_message(BackendState::Loading, 5_000)
//         .await
//         .unwrap();
//     state_subscription
//         .expect_backend_status_message(BackendState::Starting, 30_000)
//         .await
//         .unwrap();
//     state_subscription
//         .expect_backend_status_message(BackendState::ErrorStarting, 5_000)
//         .await
//         .unwrap();
// }

// #[integration_test]
// async fn handle_failure_after_ready() {
//     let nats = Nats::new().await.unwrap();
//     let connection = nats.connection().await.unwrap();
//     let mut controller_mock = MockController::new(connection.clone()).await.unwrap();
//     let drone_id = DroneId::new_random();
//     let agent = Agent::new(&nats, &drone_id).await.unwrap();
//     controller_mock
//         .expect_drone_status(&drone_id, agent.ip)
//         .await
//         .unwrap();

//     controller_mock
//         .expect_backend_status(&drone_id, &ClusterName::new("plane.test"), true, 0)
//         .await
//         .unwrap();

//     let mut request = base_spawn_request();
//     request.drone_id = drone_id;

//     let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
//         .await
//         .unwrap();
//     controller_mock.spawn_backend(&request).await.unwrap();

//     state_subscription
//         .wait_for_state(BackendState::Ready, 60_000)
//         .await
//         .unwrap();

//     let proxy_route = agent
//         .db
//         .get_proxy_route(request.backend_id.id())
//         .await
//         .unwrap()
//         .expect("Expected proxy route.");
//     // A get request to this URL will cause the container to exit with status 1.
//     // We don't check the status, because the request itself is expected to fail
//     // (the process exits immediately, so the response is not sent).
//     let _ = reqwest::get(format!("http://{}/exit/1", proxy_route)).await;

//     state_subscription
//         .expect_backend_status_message(BackendState::Failed, 5_000)
//         .await
//         .unwrap();
// }

// #[integration_test]
// async fn handle_successful_termination() {
//     let nats = Nats::new().await.unwrap();
//     let connection = nats.connection().await.unwrap();
//     let mut controller_mock = MockController::new(connection.clone()).await.unwrap();
//     let drone_id = DroneId::new_random();
//     let agent = Agent::new(&nats, &drone_id).await.unwrap();
//     controller_mock
//         .expect_drone_status(&drone_id, agent.ip)
//         .await
//         .unwrap();

//     controller_mock
//         .expect_backend_status(&drone_id, &ClusterName::new("plane.test"), true, 0)
//         .await
//         .unwrap();

//     let mut request = base_spawn_request();
//     request.drone_id = drone_id;

//     let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
//         .await
//         .unwrap();
//     controller_mock.spawn_backend(&request).await.unwrap();

//     state_subscription
//         .wait_for_state(BackendState::Ready, 60_000)
//         .await
//         .unwrap();

//     let proxy_route = agent
//         .db
//         .get_proxy_route(request.backend_id.id())
//         .await
//         .unwrap()
//         .expect("Expected proxy route.");
//     // A get request to this URL will cause the container to exit with status 1.
//     // We don't check the status, because the request itself is expected to fail
//     // (the process exits immediately, so the response is not sent).
//     let _ = reqwest::get(format!("http://{}/exit/0", proxy_route)).await;

//     state_subscription
//         .expect_backend_status_message(BackendState::Exited, 5_000)
//         .await
//         .unwrap();
// }

// #[integration_test]
// async fn handle_agent_restart() {
//     let nats_con = Nats::new().await.unwrap();
//     let nats = nats_con.connection().await.unwrap();
//     let mut controller_mock = MockController::new(nats.clone()).await.unwrap();

//     let mut state_subscription = {
//         let drone_id = DroneId::new_random();
//         let agent = Agent::new(&nats_con, &drone_id).await.unwrap();
//         controller_mock
//             .expect_drone_status(&drone_id, agent.ip)
//             .await
//             .unwrap();

//         controller_mock
//             .expect_backend_status(&drone_id, &ClusterName::new("plane.test"), true, 0)
//             .await
//             .unwrap();

//         let mut request = base_spawn_request();
//         request.drone_id = drone_id;

//         let mut state_subscription = BackendStateSubscription::new(&nats, &request.backend_id)
//             .await
//             .unwrap();
//         controller_mock.spawn_backend(&request).await.unwrap();
//         state_subscription
//             .wait_for_state(BackendState::Ready, 60_000)
//             .await
//             .unwrap();
//         state_subscription
//     };

//     // Original agent goes away when it goes out of scope.
//     {
//         let drone_id = DroneId::new_random();
//         let agent = Agent::new(&nats_con, &drone_id).await.unwrap();
//         controller_mock
//             .expect_drone_status(&drone_id, agent.ip)
//             .await
//             .unwrap();

//         state_subscription
//             .wait_for_state(BackendState::Swept, 20_000)
//             .await
//             .unwrap();
//     }
// }

// #[integration_test]
// async fn handle_termination_request() {
//     let nats = Nats::new().await.unwrap();
//     let connection = nats.connection().await.unwrap();
//     let mut controller_mock = MockController::new(connection.clone()).await.unwrap();
//     let drone_id = DroneId::new_random();
//     let agent = Agent::new(&nats, &drone_id).await.unwrap();

//     let mut request = base_spawn_request();
//     // Ensure spawnee lives long enough to be terminated.
//     request.drone_id = drone_id.clone();
//     request.max_idle_secs = Duration::from_secs(10_000);

//     controller_mock
//         .expect_drone_status(&drone_id, agent.ip)
//         .await
//         .unwrap();
//     controller_mock
//         .expect_backend_status(&request.drone_id, &ClusterName::new("plane.test"), true, 0)
//         .await
//         .unwrap();

//     request.max_idle_secs = Duration::from_secs(1000);
//     let mut state_subscription = BackendStateSubscription::new(&connection, &request.backend_id)
//         .await
//         .unwrap();

//     controller_mock.spawn_backend(&request).await.unwrap();
//     state_subscription
//         .wait_for_state(BackendState::Ready, 20_000)
//         .await
//         .unwrap();

//     let termination_request = TerminationRequest {
//         backend_id: request.backend_id.clone(),
//         cluster_id: ClusterName::new(CLUSTER_DOMAIN),
//     };
//     controller_mock
//         .terminate_backend(&termination_request)
//         .await
//         .unwrap();

//     state_subscription
//         .wait_for_state(BackendState::Terminated, 20_000)
//         .await
//         .unwrap();
// }
