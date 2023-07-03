use anyhow::Result;
use chrono::{DateTime, Utc};
use plane_core::{
    state::StateHandle,
    types::{ClusterName, DroneId},
};
use rand::{seq::SliceRandom, thread_rng};
use std::{error::Error, fmt::Display};

#[derive(Clone)]
pub struct Scheduler {
    state: StateHandle,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SchedulerError {
    NoDroneAvailable,
}

impl Display for SchedulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for SchedulerError {}

impl Scheduler {
    pub fn new(state: StateHandle) -> Self {
        Self { state }
    }

    pub fn schedule(
        &self,
        cluster: &ClusterName,
        current_timestamp: DateTime<Utc>,
    ) -> Result<DroneId, SchedulerError> {
        self.state
            .get_ready_drones(cluster, current_timestamp)
            .map_err(|error| {
                tracing::error!(?error, "Could not get ready drones.");
                SchedulerError::NoDroneAvailable
            })?
            .choose(&mut thread_rng())
            .cloned()
            .ok_or(SchedulerError::NoDroneAvailable)
    }
}
