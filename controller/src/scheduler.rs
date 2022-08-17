use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use dis_spawner::{messages::agent::DroneStatusMessage, types::DroneId};
use rand::{seq::SliceRandom, thread_rng};
use std::{error::Error, fmt::Display};

#[derive(Default)]
pub struct Scheduler {
    last_status: DashMap<DroneId, DateTime<Utc>>,
}

#[derive(Debug)]
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
    pub fn update_status(&self, timestamp: DateTime<Utc>, status: &DroneStatusMessage) {
        self.last_status.insert(status.drone_id, timestamp);
    }

    pub fn schedule(&self) -> Result<DroneId, SchedulerError> {
        // TODO: this is a dumb placeholder scheduler.

        let threshold_time = Utc::now().checked_sub_signed(Duration::seconds(5)).unwrap();

        let drone_ids: Vec<DroneId> = self
            .last_status
            .iter()
            .filter(|d| d.value() > &threshold_time)
            .map(|d| *d.key())
            .collect();

        drone_ids
            .choose(&mut thread_rng())
            .cloned()
            .ok_or(SchedulerError::NoDroneAvailable)
    }
}
