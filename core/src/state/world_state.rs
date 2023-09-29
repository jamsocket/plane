use crate::{
    messages::{
        agent,
        state::{
            BackendLockAssignment, BackendMessageType, ClusterLockMessageType, ClusterStateMessage,
            DroneMessageType, DroneMeta, WorldStateMessage,
        },
    },
    types::{BackendId, ClusterName, DroneId, LockState, ResourceLock},
};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::{
    cmp::Ordering,
    collections::{btree_map::Entry, BTreeMap, BTreeSet, BinaryHeap, VecDeque},
    fmt::Debug,
    future::ready,
    net::IpAddr,
    pin::Pin,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use tokio::sync::broadcast::{channel, Receiver, Sender};

const LOCK_ANNOUNCE_PERIOD_SECONDS: i64 = 30;

#[derive(Default, Debug, Clone)]
pub struct StateHandle {
    state: Arc<RwLock<WorldState>>,
}

impl StateHandle {
    pub fn new(world_state: WorldState) -> Self {
        Self {
            state: Arc::new(RwLock::new(world_state)),
        }
    }

    pub fn wait_for_seq(
        // note: this is exclusively borrowed to prevent deadlocks
        // that can result from awaiting wait_for_seq
        // while holding a handle to .state()
        &mut self,
        sequence: u64,
    ) -> Pin<Box<dyn core::future::Future<Output = ()> + Send + Sync>> {
        let mut state = self.state.write().unwrap();
        if state.logical_time >= sequence {
            return Box::pin(ready(()));
        }
        let mut recv = state.get_listener(sequence);
        Box::pin(async move {
            recv.recv().await.unwrap();
        })
    }

    pub fn get_ready_drones(
        &self,
        cluster: &ClusterName,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<DroneId>> {
        let world_state = self
            .state
            .read()
            .expect("Could not acquire world_state lock.");

        let min_keepalive = timestamp - chrono::Duration::seconds(30);

        Ok(world_state
            .cluster(cluster)
            .ok_or_else(|| anyhow!("Cluster not found."))?
            .drones
            .iter()
            .filter(|(_, drone)| {
                drone.state() == Some(agent::DroneState::Ready)
                    && drone.meta.is_some()
                    && drone.last_seen > Some(min_keepalive)
            })
            .map(|(drone_id, _)| drone_id.clone())
            .collect())
    }

    pub fn state(&self) -> RwLockReadGuard<WorldState> {
        self.state
            .read()
            .expect("Could not acquire world_state lock.")
    }

    pub(crate) fn write_state(&self) -> RwLockWriteGuard<WorldState> {
        self.state
            .write()
            .expect("Could not acquire world_state lock.")
    }
}

#[derive(Debug)]
struct RemoveLockEvent {
    uid: u64,
    lock: ResourceLock,
    time: DateTime<Utc>,
}

impl RemoveLockEvent {
    fn new(time: DateTime<Utc>, lock: ResourceLock, uid: u64) -> Self {
        Self { uid, lock, time }
    }
}

impl PartialEq for RemoveLockEvent {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl Eq for RemoveLockEvent {}

impl PartialOrd for RemoveLockEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(Ordering::reverse(self.time.cmp(&other.time)))
    }
}

impl Ord for RemoveLockEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        Ordering::reverse(self.time.cmp(&other.time))
    }
}

#[derive(Debug)]
struct SequenceListener {
    sender: Sender<()>,
    seq: u64,
}

impl SequenceListener {
    fn new(seq: u64) -> Self {
        let (sender, _recv) = channel(1);
        Self { sender, seq }
    }
}

impl PartialEq for SequenceListener {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}

impl Eq for SequenceListener {}

impl PartialOrd for SequenceListener {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(Ordering::reverse(self.seq.cmp(&other.seq)))
    }
}

impl Ord for SequenceListener {
    fn cmp(&self, other: &Self) -> Ordering {
        Ordering::reverse(self.seq.cmp(&other.seq))
    }
}

#[derive(Debug, Default)]
pub struct WorldState {
    logical_time: u64,
    clock_time: DateTime<Utc>,
    pub clusters: BTreeMap<ClusterName, ClusterState>,
    listeners: BinaryHeap<SequenceListener>,
}

impl WorldState {
    pub fn logical_time(&self) -> u64 {
        self.logical_time
    }

    pub fn clock_time(&self) -> DateTime<Utc> {
        self.clock_time
    }

    fn get_listener(&mut self, sequence: u64) -> Receiver<()> {
        let listener = SequenceListener::new(sequence);
        let recv = listener.sender.subscribe();
        self.listeners.push(listener);
        recv
    }

    fn notify_listeners(&mut self) {
        while self
            .listeners
            .peek()
            .is_some_and(|l| l.seq <= self.logical_time)
        {
            let listener = self.listeners.pop().unwrap();
            if let Err(e) = listener.sender.send(()) {
                tracing::info!(?self.logical_time, ?e, "failed to notify listener at seq");
            }
        }
    }

    pub fn apply(&mut self, message: WorldStateMessage, sequence: u64, timestamp: DateTime<Utc>) {
        match message {
            WorldStateMessage::ClusterMessage { cluster, message } => {
                let cluster = self.clusters.entry(cluster).or_default();
                cluster.apply(message, timestamp);
            }
            WorldStateMessage::Heartbeat { .. } => {
                self.clock_time = timestamp;
                for cluster in self.clusters.values_mut() {
                    cluster.process_lock_announce_expiration(&timestamp);
                }
            }
        }
        self.logical_time = sequence;
        self.notify_listeners();
    }

    pub fn cluster(&self, cluster: &ClusterName) -> Option<&ClusterState> {
        self.clusters.get(cluster)
    }
}

#[derive(Default, Debug)]
pub struct ClusterState {
    pub drones: BTreeMap<DroneId, DroneState>,
    pub backends: BTreeMap<BackendId, BackendState>,
    pub txt_records: VecDeque<String>,
    pub locks: BTreeMap<ResourceLock, LockState>,
    lock_announce_expiration_queue: BinaryHeap<RemoveLockEvent>,
}

impl ClusterState {
    fn revoke_lock(&mut self, lock: ResourceLock, lock_uid: u64) {
        match self.locks.entry(lock) {
            Entry::Vacant(entry) => {
                let lock = entry.key();
                tracing::info!(?lock, "requested revocation of nonexistent lock");
            }
            Entry::Occupied(entry) => {
                if matches!(
                    entry.get(),
                    LockState::Announced { uid } if *uid == lock_uid
                ) {
                    let (lock, _) = entry.remove_entry();
                    tracing::info!(?lock, "announced lock revoked");
                }
            }
        }
    }
    fn apply(&mut self, message: ClusterStateMessage, timestamp: DateTime<Utc>) {
        match message {
            ClusterStateMessage::LockMessage(message) => match message.message {
                ClusterLockMessageType::Announce => match self.locks.entry(message.lock) {
                    Entry::Vacant(entry) => {
                        self.lock_announce_expiration_queue
                            .push(RemoveLockEvent::new(
                                timestamp + chrono::Duration::seconds(LOCK_ANNOUNCE_PERIOD_SECONDS),
                                entry.key().clone(),
                                message.uid,
                            ));
                        entry.insert(LockState::Announced { uid: message.uid });
                    }
                    Entry::Occupied(entry) => {
                        let lock = entry.key();
                        let lock_state = entry.get();
                        tracing::info!(?lock, ?lock_state, "lock already exists in map");
                    }
                },
                ClusterLockMessageType::Revoke => {
                    self.revoke_lock(message.lock, message.uid);
                }
            },
            ClusterStateMessage::DroneMessage(message) => {
                let drone = self.drones.entry(message.drone).or_default();
                drone.apply(message.message);
            }
            ClusterStateMessage::BackendMessage(message) => {
                let backend = self.backends.entry(message.backend.clone()).or_default();

                // If the message is a lock assignment, we want to record it.
                if let BackendMessageType::Assignment {
                    lock_assignment:
                        Some(BackendLockAssignment {
                            ref lock,
                            uid: assigned_uid,
                        }),
                    ..
                } = message.message
                {
                    match self.locks.entry(lock.clone()) {
                        Entry::Vacant(_) => {
                            tracing::error!(?lock, "lock must be announced before assignment");
                            //bail out, and don't apply message to backend
                            return;
                        }
                        Entry::Occupied(v)
                            if matches!(
                                *v.get(), LockState::Announced { uid } if assigned_uid != uid
                            ) =>
                        {
                            tracing::error!(?lock, "lock must have same uid to assign");
                            return;
                        }
                        Entry::Occupied(mut v) => {
                            *v.get_mut() = LockState::Assigned {
                                backend: message.backend,
                            }
                        }
                    }
                }

                // If the backend is terminal, remove locks from map
                if let BackendMessageType::State { state, .. } = message.message {
                    if state.terminal() && backend.lock.is_some() {
                        let lock = backend.lock.as_ref().unwrap();
                        self.locks.remove(lock);
                    }
                }

                backend.apply(message.message);
            }
            ClusterStateMessage::AcmeMessage(message) => {
                if self.txt_records.len() >= 10 {
                    self.txt_records.pop_front();
                }

                self.txt_records.push_back(message.value);
            }
        }
    }

    fn process_lock_announce_expiration(&mut self, clock_time: &DateTime<Utc>) {
        while self
            .lock_announce_expiration_queue
            .peek()
            .is_some_and(|l| l.time <= *clock_time)
        {
            let remove_lock_evt = self.lock_announce_expiration_queue.pop().unwrap();
            self.revoke_lock(remove_lock_evt.lock, remove_lock_evt.uid);
        }
    }

    pub fn a_record_lookup(&self, backend: &BackendId) -> Option<IpAddr> {
        let backend = self.backends.get(backend)?;

        let drone = backend.drone.as_ref()?;
        let drone = self.drones.get(drone)?;

        drone.meta.as_ref().map(|d| d.ip)
    }

    pub fn drone(&self, drone: &DroneId) -> Option<&DroneState> {
        self.drones.get(drone)
    }

    pub fn backend(&self, backend: &BackendId) -> Option<&BackendState> {
        self.backends.get(backend)
    }

    pub fn locked(&self, lock: &ResourceLock) -> LockState {
        if let Some(lock_state) = self.locks.get(lock) {
            lock_state.clone()
        } else {
            LockState::Unlocked
        }
    }
}

#[derive(Default, Debug)]
pub struct DroneState {
    pub meta: Option<DroneMeta>,
    pub states: BTreeSet<(chrono::DateTime<chrono::Utc>, agent::DroneState)>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
}

impl DroneState {
    fn apply(&mut self, message: DroneMessageType) {
        match message {
            DroneMessageType::Metadata(meta) => self.meta = Some(meta),
            DroneMessageType::State { state, timestamp } => {
                self.states.insert((timestamp, state));
            }
            DroneMessageType::KeepAlive { timestamp } => self.last_seen = Some(timestamp),
        }
    }

    /// Return the most recent state, or None.
    pub fn state(&self) -> Option<agent::DroneState> {
        self.states.last().map(|(_, state)| *state)
    }
}

#[derive(Default, Debug)]
pub struct BackendState {
    pub drone: Option<DroneId>,
    pub bearer_token: Option<String>,
    pub lock: Option<ResourceLock>,
    pub states: BTreeSet<(chrono::DateTime<chrono::Utc>, agent::BackendState)>,
}

impl BackendState {
    fn apply(&mut self, message: BackendMessageType) {
        match message {
            BackendMessageType::Assignment {
                drone,
                bearer_token,
                lock_assignment,
            } => {
                self.drone = Some(drone);
                self.bearer_token = bearer_token;
                self.lock = lock_assignment.map(|la| la.lock)
            }
            BackendMessageType::State {
                state: status,
                timestamp,
            } => {
                self.states.insert((timestamp, status));
                if status.terminal() {
                    self.lock = None
                }
            }
        }
    }

    pub fn state(&self) -> Option<agent::BackendState> {
        self.states.last().map(|(_, state)| *state)
    }

    pub fn state_timestamp(&self) -> Option<(chrono::DateTime<chrono::Utc>, agent::BackendState)> {
        self.states.last().copied()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::messages::state::{
        AcmeDnsRecord, BackendMessage, ClusterLockMessage, ClusterLockMessageType,
    };
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_listener() {
        let mut state = StateHandle::default();
        let now = chrono::Utc::now();

        let zerolistener = state.wait_for_seq(0);
        timeout(Duration::from_secs(0), zerolistener)
            .await
            .expect("zerolistener should return immediately");

        let onelistener = state.wait_for_seq(1);
        let onelistener_2 = state.wait_for_seq(1);
        let twolistener = state.wait_for_seq(2);
        let twolistener_2 = state.wait_for_seq(2);
        // onelistener should block.
        {
            let result = timeout(Duration::from_secs(0), onelistener).await;
            assert!(result.is_err());
        }

        state.write_state().apply(
            WorldStateMessage::ClusterMessage {
                cluster: ClusterName::new("cluster"),
                message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                    value: "value".into(),
                }),
            },
            1,
            now,
        );

        // onelistener should now return.
        timeout(Duration::from_secs(0), onelistener_2)
            .await
            .expect("onelistener should return");

        // twolistener should block
        {
            let result = timeout(Duration::from_secs(0), twolistener).await;
            assert!(result.is_err());
        }

        state.write_state().apply(
            WorldStateMessage::ClusterMessage {
                cluster: ClusterName::new("cluster"),
                message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                    value: "value".into(),
                }),
            },
            2,
            now,
        );

        // twolistener should now return.
        {
            timeout(Duration::from_secs(0), twolistener_2)
                .await
                .expect("wait_for_seq(2) should return immediately");
        }
    }

    #[test]
    fn test_locks() {
        let mut state = ClusterState::default();
        let backend = BackendId::new_random();
        let now = chrono::Utc::now();
        let mylock: ResourceLock = "mylock".to_string().try_into().unwrap();

        // Initially, no locks are held.
        assert_eq!(state.locked(&mylock), LockState::Unlocked);

        // Announce a lock
        state.apply(
            ClusterStateMessage::LockMessage(ClusterLockMessage {
                uid: 1,
                lock: mylock.clone(),
                message: ClusterLockMessageType::Announce,
            }),
            now,
        );

        assert_eq!(state.locked(&mylock), LockState::Announced { uid: 1 });

        // Assign a backend to a drone and acquire a lock.
        state.apply(
            ClusterStateMessage::BackendMessage(BackendMessage {
                backend: backend.clone(),
                message: BackendMessageType::Assignment {
                    drone: DroneId::new("drone".into()),
                    bearer_token: None,
                    lock_assignment: Some(BackendLockAssignment {
                        lock: mylock.clone(),
                        uid: 1,
                    }),
                },
            }),
            now,
        );

        assert_eq!(
            state.locked(&mylock),
            LockState::Assigned {
                backend: backend.clone()
            }
        );

        // Update the backend state to loading.
        let now = Utc::now();
        state.apply(
            ClusterStateMessage::BackendMessage(BackendMessage {
                backend: backend.clone(),
                message: BackendMessageType::State {
                    state: agent::BackendState::Loading,
                    timestamp: now,
                },
            }),
            now,
        );

        // Update the backend state to starting
        state.apply(
            ClusterStateMessage::BackendMessage(BackendMessage {
                backend: backend.clone(),
                message: BackendMessageType::State {
                    state: agent::BackendState::Starting,
                    timestamp: now,
                },
            }),
            now,
        );

        assert_eq!(
            state.locked(&mylock),
            LockState::Assigned {
                backend: backend.clone()
            }
        );

        // Update the backend state to ready.
        state.apply(
            ClusterStateMessage::BackendMessage(BackendMessage {
                backend: backend.clone(),
                message: BackendMessageType::State {
                    state: agent::BackendState::Ready,
                    timestamp: Utc::now(),
                },
            }),
            now,
        );

        assert_eq!(
            state.locked(&mylock),
            LockState::Assigned {
                backend: backend.clone()
            }
        );

        // Update the backend state to swept.
        state.apply(
            ClusterStateMessage::BackendMessage(BackendMessage {
                backend,
                message: BackendMessageType::State {
                    state: agent::BackendState::Swept,
                    timestamp: Utc::now(),
                },
            }),
            now,
        );

        // The lock should now be free.
        assert_eq!(state.locked(&mylock), LockState::Unlocked);

        state.backends.remove(&BackendId::new("backend".into()));

        // The lock should still be free.
        assert_eq!(state.locked(&mylock), LockState::Unlocked);
    }

    #[tokio::test]
    async fn test_lock_announce_timeout() {
        let state = StateHandle::default();
        let now = chrono::Utc::now();
        let mylock: ResourceLock = "mylock".to_string().try_into().unwrap();

        state
            .write_state()
            .apply(WorldStateMessage::Heartbeat { heartbeat: None }, 1, now);
        state.write_state().apply(
            WorldStateMessage::ClusterMessage {
                cluster: ClusterName::new("cluster"),
                message: ClusterStateMessage::AcmeMessage(AcmeDnsRecord {
                    value: "value".into(),
                }),
            },
            2,
            now,
        );
        state.write_state().apply(
            WorldStateMessage::ClusterMessage {
                cluster: ClusterName::new("cluster"),
                message: ClusterStateMessage::LockMessage(ClusterLockMessage {
                    lock: mylock.clone(),
                    uid: 1,
                    message: ClusterLockMessageType::Announce,
                }),
            },
            3,
            now,
        );

        assert_eq!(
            state
                .state()
                .cluster(&ClusterName::new("cluster"))
                .unwrap()
                .locked(&mylock),
            LockState::Announced { uid: 1 }
        );

        let now = now + chrono::Duration::seconds(30);
        state
            .write_state()
            .apply(WorldStateMessage::Heartbeat { heartbeat: None }, 4, now);

        assert_eq!(
            state
                .state()
                .cluster(&ClusterName::new("cluster"))
                .unwrap()
                .locked(&mylock),
            LockState::Unlocked
        );
    }
}
