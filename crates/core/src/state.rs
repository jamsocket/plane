use std::fmt::{Debug, Display};

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum SessionLivedBackendState {
    /// The `SessionLivedBackend` object exists.
    Submitted,

    /// The pod and service backing a `SessionLivedBackend` exist.
    Constructed,

    /// The pod that backs a `SessionLivedBackend` has been assigned to a node.
    Scheduled,

    /// The pod that backs a `SessionLivedBackend` is running.
    Running,

    /// The pod that backs a `SessionLivedBackend` is accepting new connections.
    Ready,

    /// The `SessionLivedBackend` has been marked as swept, meaning that it can be deleted.
    Swept,

    /// The backend failed.
    Failed,
}

impl Default for SessionLivedBackendState {
    fn default() -> Self {
        SessionLivedBackendState::Submitted
    }
}

impl Display for SessionLivedBackendState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self, f)
    }
}

impl SessionLivedBackendState {
    pub fn message(&self) -> String {
        match self {
            SessionLivedBackendState::Submitted => {
                "SessionLivedBackend object created.".to_string()
            }
            SessionLivedBackendState::Constructed => {
                "Backing resources created by Spawner.".to_string()
            }
            SessionLivedBackendState::Scheduled => {
                "Backing pod was scheduled by Kubernetes.".to_string()
            }
            SessionLivedBackendState::Running => "Pod was observed running.".to_string(),
            SessionLivedBackendState::Ready => {
                "Pod was observed listening on TCP port.".to_string()
            }
            SessionLivedBackendState::Swept => {
                "SessionLivedBackend was found idle and swept.".to_string()
            }
            &SessionLivedBackendState::Failed => "SessionLivedBackend failed.".to_string(),
        }
    }
}
