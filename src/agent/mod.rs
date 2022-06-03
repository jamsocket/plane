use std::net::IpAddr;

#[derive(PartialEq, Debug)]
pub enum DockerApiTransport {
    Socket(String),
    Http(String),
}

impl Default for DockerApiTransport {
    fn default() -> Self {
        DockerApiTransport::Socket("/var/run/docker.sock".to_string())
    }
}

#[derive(PartialEq, Debug)]
pub struct DockerOptions {
    pub transport: DockerApiTransport,
    pub runtime: Option<String>,
}

#[derive(PartialEq, Debug)]
pub struct AgentOptions {
    pub db_path: String,
    pub nats_url: String,
    pub cluster_domain: String,

    /// Public IP of the machine the drone is running on.
    pub ip: IpAddr,

    /// Internal IP of the machine the drone is running on.
    pub host_ip: IpAddr,

    pub docker_options: DockerOptions,
}
