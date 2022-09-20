use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use dis_spawner::{
    messages::agent::DroneStatusMessage,
    types::{ClusterName, DroneId},
};
use rand::{seq::SliceRandom, thread_rng};
use std::{error::Error, fmt::Display};

#[derive(Default)]
pub struct Scheduler {
    last_status: DashMap<ClusterName, DashMap<DroneId, DateTime<Utc>>>,
}

#[derive(Debug, PartialEq)]
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
        self.last_status
            .entry(status.cluster.clone())
            .or_default()
            .insert(status.drone_id.clone(), timestamp);
    }

    pub fn schedule(
        &self,
        cluster: &ClusterName,
        current_timestamp: DateTime<Utc>,
    ) -> Result<DroneId, SchedulerError> {
        // TODO: this is a dumb placeholder scheduler.

        let threshold_time = current_timestamp
            .checked_sub_signed(Duration::seconds(5))
            .unwrap();

        let drone_ids: Vec<DroneId> = self
            .last_status
            .get(cluster)
            .ok_or(SchedulerError::NoDroneAvailable)?
            .iter()
            .filter(|d| d.value() > &threshold_time)
            .map(|d| d.key().clone())
            .collect();

        drone_ids
            .choose(&mut thread_rng())
            .cloned()
            .ok_or(SchedulerError::NoDroneAvailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(date: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(date).unwrap().into()
    }

    #[test]
    fn test_no_drones() {
        let scheduler = Scheduler::default();
        let timestamp = date("2020-01-01T05:00:00+00:00");
        assert_eq!(
            Err(SchedulerError::NoDroneAvailable),
            scheduler.schedule(&ClusterName::new("mycluster.test"), timestamp)
        );
    }

    #[test]
    fn test_one_drone() {
        let scheduler = Scheduler::default();
        let drone_id = DroneId::new_random();

        scheduler.update_status(
            date("2020-01-01T05:00:00+00:00"),
            &DroneStatusMessage {
                drone_id: drone_id.clone(),
                cluster: ClusterName::new("mycluster.test"),
                capacity: 100,
            },
        );

        assert_eq!(
            Ok(drone_id),
            scheduler.schedule(
                &ClusterName::new("mycluster.test"),
                date("2020-01-01T05:00:03+00:00")
            )
        );
    }

    #[test]
    fn test_one_drone_wrong_cluster() {
        let scheduler = Scheduler::default();

        scheduler.update_status(
            date("2020-01-01T05:00:00+00:00"),
            &DroneStatusMessage {
                drone_id: DroneId::new_random(),
                cluster: ClusterName::new("mycluster1.test"),
                capacity: 100,
            },
        );

        assert_eq!(
            Err(SchedulerError::NoDroneAvailable),
            scheduler.schedule(
                &ClusterName::new("mycluster2.test"),
                date("2020-01-01T05:00:03+00:00")
            )
        );
    }

    #[test]
    fn test_one_drone_expired() {
        let scheduler = Scheduler::default();

        scheduler.update_status(
            date("2020-01-01T05:00:00+00:00"),
            &DroneStatusMessage {
                drone_id: DroneId::new_random(),
                cluster: ClusterName::new("mycluster.test"),
                capacity: 100,
            },
        );

        assert_eq!(
            Err(SchedulerError::NoDroneAvailable),
            scheduler.schedule(
                &ClusterName::new("mycluster.test"),
                date("2020-01-01T05:00:09+00:00")
            )
        );
    }
}
