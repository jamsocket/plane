use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt::Display, net::SocketAddr};

use crate::log_types::BackendAddr;

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, PartialOrd, valuable::Valuable)]
pub enum BackendStatus {
    /// The backend has been scheduled to a drone, but has not yet been acknowledged.
    /// This status is only assigned by the controller; the drone will never assign it by definition.
    Scheduled,

    /// The backend has been assigned to a drone, which is now responsible for loading its image.
    Loading,

    /// Telling Docker to start the container.
    Starting,

    /// Wait for the backend to be ready to accept connections.
    Waiting,

    /// The backend is listening for connections.
    Ready,

    /// The backend has been sent a SIGTERM, either because we sent it or the user did,
    /// and we are waiting for it to exit.
    /// Proxies should stop sending traffic to it, but we should not yet release the key.
    Terminating,

    /// The backend has exited or been swept.
    Terminated,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, valuable::Valuable)]
pub enum TerminationKind {
    Soft,
    Hard,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, valuable::Valuable)]
pub struct BackendState {
    pub status: BackendStatus,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<BackendAddr>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination: Option<TerminationKind>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<TerminationReason>,

    /// The last status before the backend entered Terminating or Terminated.
    pub last_status: Option<BackendStatus>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, valuable::Valuable)]
pub enum TerminationReason {
    Swept,
    External,
    KeyExpired,
}

impl BackendState {
    pub fn address(&self) -> Option<SocketAddr> {
        self.address.as_ref().map(|BackendAddr(addr)| *addr)
    }

    pub fn to_loading(&self) -> BackendState {
        BackendState {
            status: BackendStatus::Loading,
            ..self.clone()
        }
    }

    pub fn to_starting(&self) -> BackendState {
        BackendState {
            status: BackendStatus::Starting,
            ..self.clone()
        }
    }

    pub fn to_waiting(&self, address: SocketAddr) -> BackendState {
        BackendState {
            status: BackendStatus::Waiting,
            address: Some(BackendAddr(address)),
            ..self.clone()
        }
    }

    pub fn to_ready(&self) -> BackendState {
        BackendState {
            status: BackendStatus::Ready,
            ..self.clone()
        }
    }

    pub fn to_terminating(
        &self,
        termination: TerminationKind,
        reason: TerminationReason,
    ) -> BackendState {
        BackendState {
            status: BackendStatus::Terminating,
            termination: Some(termination),
            reason: Some(reason),
            last_status: Some(self.status),
            address: None,
            exit_code: None,
        }
    }

    pub fn to_terminated(&self, exit_code: Option<i32>) -> BackendState {
        BackendState {
            status: BackendStatus::Terminated,
            address: None,
            last_status: self.last_status.or(Some(self.status)),
            exit_code,
            ..self.clone()
        }
    }
}

impl Default for BackendState {
    fn default() -> Self {
        Self {
            status: BackendStatus::Scheduled,
            address: None,
            termination: None,
            reason: None,
            last_status: None,
            exit_code: None,
        }
    }
}

impl TryFrom<String> for BackendStatus {
    type Error = serde_json::Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        serde_json::from_value(Value::String(s))
    }
}

impl Display for BackendStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let result = serde_json::to_value(self);
        match result {
            Ok(Value::String(v)) => write!(f, "{}", v),
            _ => unreachable!(),
        }
    }
}
