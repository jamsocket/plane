
export interface DroneConnectRequest {
    cluster: string
    ip: string
}

export interface SpawnRequest {
    image: string
    backend_id: string
    max_idle_secs: number
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