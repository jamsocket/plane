use integration_test::integration_test;
use plane_controller::{
    drone_state::monitor_drone_state,
    state::{start_state_loop, StateHandle},
};
use plane_core::{
    messages::cert::SetAcmeDnsRecord, nats::TypedNats, types::ClusterName, NeverResult,
};
use plane_dev::{
    resources::nats::Nats,
    timeout::{expect_to_stay_alive, LivenessGuard},
};

struct StateTestFixture {
    _nats: Nats,
    nats: TypedNats,
    pub state: StateHandle,
    _lg: LivenessGuard<NeverResult>,
}

impl StateTestFixture {
    async fn new() -> Self {
        let nats = Nats::new().await.unwrap();
        let conn = nats.connection().await.unwrap();
        let state = start_state_loop(conn.clone()).await.unwrap();
        let lg = expect_to_stay_alive(monitor_drone_state(conn.clone()));

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        Self {
            _nats: nats,
            state,
            nats: conn,
            _lg: lg,
        }
    }
}

#[integration_test]
async fn txt_record_from_drone() {
    let fixture = StateTestFixture::new().await;

    {
        let result = fixture
            .nats
            .request(&SetAcmeDnsRecord {
                cluster: ClusterName::new("plane.test".into()),
                value: "test123".into(),
            })
            .await
            .unwrap();

        assert!(result);

        let state = fixture.state.state();
        let cluster = state
            .cluster(&ClusterName::new("plane.test".into()))
            .unwrap();
        assert_eq!(cluster.txt_records.len(), 1);
        assert_eq!(cluster.txt_records.back().unwrap(), "test123");

        // NB: it's important that we (implicitly) drop state here,
        // because otherwise we will hit a deadlock when the state
        // synchronizer tries to acquire the lock.
    }

    {
        let result = fixture
            .nats
            .request(&SetAcmeDnsRecord {
                cluster: ClusterName::new("plane.test".into()),
                value: "test456".into(),
            })
            .await
            .unwrap();

        assert!(result);

        let state = fixture.state.state();
        let cluster = state
            .cluster(&ClusterName::new("plane.test".into()))
            .unwrap();
        assert_eq!(cluster.txt_records.len(), 2);
        assert_eq!(cluster.txt_records.back().unwrap(), "test456");
    }
}

#[integration_test]
async fn txt_records_different_clusters() {
    let fixture = StateTestFixture::new().await;

    let result = fixture
        .nats
        .request(&SetAcmeDnsRecord {
            cluster: ClusterName::new("plane.test".into()),
            value: "test123".into(),
        })
        .await
        .unwrap();
    assert!(result);
    let result = fixture
        .nats
        .request(&SetAcmeDnsRecord {
            cluster: ClusterName::new("abc.test".into()),
            value: "test456".into(),
        })
        .await
        .unwrap();
    assert!(result);
    let state = fixture.state.state();

    {
        let cluster = state
            .cluster(&ClusterName::new("plane.test".into()))
            .unwrap();
        assert_eq!(cluster.txt_records.len(), 1);
        assert_eq!(cluster.txt_records.back().unwrap(), "test123");
    }

    {
        let cluster = state.cluster(&ClusterName::new("abc.test".into())).unwrap();
        assert_eq!(cluster.txt_records.len(), 1);
        assert_eq!(cluster.txt_records.back().unwrap(), "test456");
    }
}
