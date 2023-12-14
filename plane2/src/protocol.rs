use crate::{
    database::backend::BackendActionMessage,
    names::{BackendActionName, BackendName},
    typed_socket::ChannelMessage,
    types::{
        BackendStatus, BearerToken, ClusterName, ExecutorConfig, KeyConfig, NodeStatus,
        SecretToken, TerminationKind,
    },
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BackendAction {
    Spawn {
        executable: ExecutorConfig,
        key: KeyConfig,
    },
    Terminate {
        kind: TerminationKind,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackendStateMessage {
    pub event_id: BackendEventId,
    pub backend_id: BackendName,
    pub status: BackendStatus,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<SocketAddr>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    pub timestamp: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BackendEventId(i64);

impl From<i64> for BackendEventId {
    fn from(i: i64) -> Self {
        Self(i)
    }
}

impl From<BackendEventId> for i64 {
    fn from(id: BackendEventId) -> Self {
        id.0
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum MessageFromDrone {
    Heartbeat { status: NodeStatus },
    BackendEvent(BackendStateMessage),
    AckAction { action_id: BackendActionName },
}

impl ChannelMessage for MessageFromDrone {
    type Reply = MessageToDrone;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum MessageToDrone {
    Action(BackendActionMessage),
    /// Acknowledge that the container has received and processed a backend event.
    AckEvent {
        event_id: BackendEventId,
    },
}

impl ChannelMessage for MessageToDrone {
    type Reply = MessageFromDrone;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RouteInfo {
    pub backend_id: BackendName,
    pub address: SocketAddr,
    pub secret_token: SecretToken,
    pub user: Option<String>,
    pub user_data: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum CertManagerRequest {
    /// Request a certificate lease for a cluster.
    CertLeaseRequest,

    /// Set the TXT record for a cluster. Fails if another proxy
    /// has more recently been granted the lease.
    SetTxtRecord { txt_value: String },

    /// Release a certificate lease for a cluster so that another
    /// proxy can request it immediately.
    ReleaseCertLease,
}

impl ChannelMessage for CertManagerRequest {
    type Reply = CertManagerResponse;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum CertManagerResponse {
    /// Acknowledge a lease request and indicate whether it was accepted.
    CertLeaseResponse { accepted: bool },

    /// Acknowledge a TXT record update and indicate whether it was accepted.
    SetTxtRecordResponse { accepted: bool },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RouteInfoRequest {
    pub token: BearerToken,
}

impl ChannelMessage for RouteInfoRequest {
    type Reply = RouteInfoResponse;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RouteInfoResponse {
    pub token: BearerToken,
    pub route_info: Option<RouteInfo>,
}

impl ChannelMessage for RouteInfoResponse {
    type Reply = RouteInfoRequest;
}

impl ChannelMessage for CertManagerResponse {
    type Reply = CertManagerRequest;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MessageFromProxy {
    RouteInfoRequest(RouteInfoRequest),
    KeepAlive(BackendName),
    CertManagerRequest(CertManagerRequest),
}

impl ChannelMessage for MessageFromProxy {
    type Reply = MessageToProxy;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MessageToProxy {
    RouteInfoResponse(RouteInfoResponse),
    CertManagerResponse(CertManagerResponse),
}

impl ChannelMessage for MessageToProxy {
    type Reply = MessageFromProxy;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MessageFromDns {
    TxtRecordRequest { cluster: ClusterName },
}

impl ChannelMessage for MessageFromDns {
    type Reply = MessageToDns;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MessageToDns {
    TxtRecordResponse {
        cluster: ClusterName,
        txt_value: Option<String>,
    },
}

impl ChannelMessage for MessageToDns {
    type Reply = MessageFromDns;
}
