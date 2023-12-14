//! Constants for the heartbeat protocol.
//!
//! These are expected to conform to some invariants:
//! ```
//! use plane2::heartbeat_consts::*;
//! assert!(HEARTBEAT_INTERVAL_SECONDS > 0);
//! assert!(UNHEALTHY_SECONDS > HEARTBEAT_INTERVAL_SECONDS,
//!   "Drones will be considered unhealthy before they are expected to send a heartbeat!");
//! assert!(HARD_TERMINATE_DEADLINE_SECONDS > SOFT_TERMINATE_DEADLINE_SECONDS,
//!   "Backends will be hard terminated before they are soft terminated!");
//! assert!(ASSUME_LOST_SECONDS > HARD_TERMINATE_DEADLINE_SECONDS,
//!   "Drones will be assumed lost before they are hard terminated!");
//! ```

use std::time::Duration;

/// How often the drone should emit a heartbeat.
pub const HEARTBEAT_INTERVAL_SECONDS: i64 = 30;

/// If we have not heard from a drone in this many seconds,
/// we will consider it unhealthy. This means that we will
/// not send new connections to backends running on it,
/// but we will not remove its locks.
pub const UNHEALTHY_SECONDS: i64 = 45;

/// If a drone's lock message has not been confirmed in
/// this many seconds, it should tell any of its backends
/// which hold locks to shut down. This ensures that if a
/// drone gets disconnected from a controller, the controller
/// will eventually be able to assume that its locks can be
/// reassigned.
pub const SOFT_TERMINATE_DEADLINE_SECONDS: i64 = 60;

/// If a drone's lock message has not been confirmed in
///
pub const HARD_TERMINATE_DEADLINE_SECONDS: i64 = 90;

/// If we have not heard from a drone in this many seconds,
/// we will consider it lost. This means that we will remove
/// its locks.
pub const ASSUME_LOST_SECONDS: i64 = 120;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS as u64);
