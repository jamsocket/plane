
export interface DroneConnectRequest {
    cluster: string
    ip: string
}

export interface Duration {
    secs: number
    nanos: number
}

export interface SpawnRequest {
    image: string
    backend_id: string
    max_idle_time: Duration
    env: Record<string, string>
    metadata: Record<string, string>
}

export type BackendStatus =
    | "Loading"
    | "ErrorLoading"
    | "Starting"
    | "ErrorStarting"
    | "Ready"
    | "TimedOutBeforeReady"
    | "Failed"
    | "Exited"
    | "Swept"

export interface BackendStateMessage {
    state: BackendStatus
    time: string
}

export interface DroneStatusMessage {
    drone_id: number,
    capacity: number,
    cluster: string,
}

export interface DnsMessage {
    cluster: string
    value: string
}