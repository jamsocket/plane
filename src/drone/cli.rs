use crate::keys::KeyCertPathPair;
use anyhow::Result;
use clap::{Parser, Subcommand};
use reqwest::Url;
use std::{net::IpAddr, path::PathBuf};

use super::{
    agent::{AgentOptions, DockerApiTransport, DockerOptions},
    proxy::{ProxyHttpsOptions, ProxyOptions},
};

#[derive(Parser)]
pub struct Opts {
    /// Path to sqlite3 database file to use for getting route information.
    ///
    /// This may be a file that does not exist. In this case, it will be created.
    #[clap(long, action)]
    pub db_path: Option<String>,

    /// The domain of the cluster that this drone serves.
    #[clap(long, action)]
    pub cluster_domain: Option<String>,

    /// Port to listen for HTTP requests on.
    #[clap(long, default_value = "80", action)]
    pub http_port: u16,

    /// Port to listen for HTTPS requests on.
    #[clap(long, default_value = "443", action)]
    pub https_port: u16,

    /// Path to read private key from.
    #[clap(long, action)]
    pub https_private_key: Option<PathBuf>,

    /// Path to read certificate from.
    #[clap(long, action)]
    pub https_certificate: Option<PathBuf>,

    /// Hostname for connecting to NATS.
    #[clap(long, action)]
    pub nats_url: Option<String>,

    /// Server to use for certificate signing.
    #[clap(long, action)]
    pub acme_server: Option<String>,

    /// Public IP of this drone, used for directing traffic outside the host.
    #[clap(long, action)]
    pub ip: Option<IpAddr>,

    /// API endpoint which returns the requestor's IP.
    #[clap(long, action)]
    pub ip_api: Option<Url>,

    /// Local IP of the host (i.e. the IP published docker ports are published on),
    /// used for proxying locally.
    #[clap(long, action)]
    pub host_ip: Option<IpAddr>,

    /// Runtime to use with docker. Default is runc; runsc is an alternative if gVisor
    /// is available.
    #[clap(long, action)]
    pub docker_runtime: Option<String>,

    /// Unix socket through which to send Docker commands.
    #[clap(long, action)]
    pub docker_socket: Option<String>,

    /// HTTP url through which to send Docker commands. Mutually exclusive with --docker-socket.
    #[clap(long, action)]
    pub docker_http: Option<String>,

    #[clap(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Migrate the database, and then exit.
    Migrate,

    /// Refresh the certificate, and the exit.
    Cert,

    /// Run one or more components as a service, indefinitely. Components are selected with --proxy, --agent, and --refresh.
    Serve {
        /// Run the proxy server.
        #[clap(long, action)]
        proxy: bool,

        /// Run the agent.
        #[clap(long, action)]
        agent: bool,

        /// Run the certificate refresh loop.
        #[clap(long, action)]
        cert_refresh: bool,
    },
}

impl Default for Command {
    fn default() -> Self {
        Command::Serve {
            proxy: true,
            agent: true,
            cert_refresh: true,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct CertOptions {
    pub cluster_domain: String,
    pub nats_url: String,
    pub key_paths: KeyCertPathPair,
    pub acme_server_url: String,
}

#[allow(unused)]
#[derive(PartialEq, Debug)]
pub enum DronePlan {
    RunService {
        proxy_options: Option<ProxyOptions>,
        agent_options: Option<AgentOptions>,
        cert_options: Option<CertOptions>,
    },
    DoMigration {
        db_path: String,
    },
    DoCertificateRefresh(CertOptions),
}

#[derive(PartialEq, Debug)]
pub enum IpProvider {
    Api(Url),
    Literal(IpAddr),
}

impl IpProvider {
    pub async fn get_ip(&self) -> Result<IpAddr> {
        match self {
            IpProvider::Literal(ip) => Ok(ip.clone()),
            IpProvider::Api(url) => {
                let result = reqwest::get(url.as_ref()).await?.text().await?;
                let ip: IpAddr = result.parse()?;
                Ok(ip)
            }
        }
    }
}

impl From<Opts> for DronePlan {
    fn from(opts: Opts) -> Self {
        let key_cert_pair = if let (Some(private_key_path), Some(certificate_path)) =
            (&opts.https_private_key, &opts.https_certificate)
        {
            Some(KeyCertPathPair {
                certificate_path: certificate_path.clone(),
                private_key_path: private_key_path.clone(),
            })
        } else {
            if opts.https_private_key.is_some() {
                panic!("Expected --https-certificate if --https-private-key is provided.")
            }
            if opts.https_certificate.is_some() {
                panic!("Expected --https-private-key if --https-certificate is provided.")
            }

            None
        };

        match opts.command.unwrap_or_default() {
            Command::Migrate => DronePlan::DoMigration {
                db_path: opts
                    .db_path
                    .expect("Expected --db-path when using migrate."),
            },
            Command::Cert => {
                DronePlan::DoCertificateRefresh(CertOptions {
                    cluster_domain: opts.cluster_domain.expect("Expected --cluster-domain when using cert command."),
                    nats_url: opts.nats_url.expect("Expected --nats-host when using cert command."),
                    key_paths: key_cert_pair.expect("Expected --https-certificate and --https-private-key to point to location to write cert and key."),
                    acme_server_url: opts.acme_server.expect("Expected --acme-server when using cert command."),
                })
            },
            Command::Serve { proxy, agent, cert_refresh } => {
                let cert_options = if cert_refresh {
                    Some(CertOptions {
                        acme_server_url: opts.acme_server.clone().expect("Expected --acme-server for certificate refreshing."),
                        cluster_domain: opts.cluster_domain.clone().expect("Expected --cluster-domain for certificate refreshing."),
                        key_paths: key_cert_pair.clone().expect("Expected --https-certificate and --https-private-key for certificate refresh."),
                        nats_url: opts.nats_url.clone().expect("Expected --nats-url."),
                    })
                } else {
                    None
                };

                let proxy_options = if proxy {
                    let https_options = key_cert_pair.map(|key_cert_pair| ProxyHttpsOptions {
                        key_paths: key_cert_pair,
                        port: opts.https_port,
                    });

                    Some(ProxyOptions {
                        cluster_domain: opts
                            .cluster_domain.clone()
                            .expect("Expected --cluster-domain for serving proxy."),
                        db_path: opts
                            .db_path
                            .clone()
                            .expect("Expected --db-path for serving proxy."),
                        http_port: opts.http_port,
                        https_options,
                    })
                } else {
                    None
                };

                let agent_options = if agent {
                    let docker_transport = if let Some(docker_socket) = opts.docker_socket {
                        DockerApiTransport::Socket(docker_socket)
                    } else if let Some(docker_http) = opts.docker_http {
                        DockerApiTransport::Http(docker_http)
                    } else {
                        DockerApiTransport::default()
                    };

                    let ip = if let Some(ip) = opts.ip {
                        IpProvider::Literal(ip)
                    } else if let Some(ip_api) = opts.ip_api {
                        IpProvider::Api(ip_api)
                    } else {
                        panic!("Expected one of --ip or --ip-api.")
                    };

                    Some(AgentOptions {
                        cluster_domain: opts.cluster_domain.clone().expect("Expected --cluster-domain for running agent."),
                        db_path: opts.db_path.clone().expect("Expected --db-path for running agent."),
                        docker_options: DockerOptions {
                            runtime: opts.docker_runtime.clone(),
                            transport: docker_transport,
                        },
                        nats_url: opts.nats_url.clone().expect("Expected --nats-url for running agent."),
                        ip,

                        host_ip: opts.host_ip.expect("Expected --host-ip for running agent.")
                    })
                } else {
                    None
                };

                if proxy_options.is_none() && agent_options.is_none() {
                    panic!("Expected at least one of --proxy, --agent, --certloop if `serve` is provided explicitly.");
                }

                DronePlan::RunService { proxy_options, agent_options, cert_options }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;

    fn parse_args(args: &[&str]) -> Result<DronePlan> {
        let mut full_args = vec!["drone"];
        full_args.extend(args.iter());
        Ok(Opts::try_parse_from(full_args)?.try_into()?)
    }

    #[test]
    fn test_migrate() {
        let opts = parse_args(&["--db-path", "mydatabase", "migrate"]).unwrap();
        assert_eq!(
            DronePlan::DoMigration {
                db_path: "mydatabase".to_string()
            },
            opts
        );
    }

    #[test]
    fn test_cert() {
        let opts = parse_args(&[
            "--db-path",
            "mydatabase",
            "--https-certificate",
            "mycert.cert",
            "--https-private-key",
            "mycert.key",
            "--nats-url",
            "nats://foo@bar",
            "--cluster-domain",
            "mydomain.test",
            "--acme-server",
            "https://acme.server/dir",
            "cert",
        ])
        .unwrap();
        assert_eq!(
            DronePlan::DoCertificateRefresh(CertOptions {
                cluster_domain: "mydomain.test".to_string(),
                nats_url: "nats://foo@bar".to_string(),
                key_paths: KeyCertPathPair {
                    private_key_path: PathBuf::from("mycert.key"),
                    certificate_path: PathBuf::from("mycert.cert"),
                },
                acme_server_url: "https://acme.server/dir".to_string(),
            }),
            opts
        );
    }

    #[test]
    fn test_proxy() {
        let opts = parse_args(&[
            "--db-path",
            "mydatabase",
            "--cluster-domain",
            "mycluster.test",
            "serve",
            "--proxy",
        ])
        .unwrap();
        assert_eq!(
            DronePlan::RunService {
                proxy_options: Some(ProxyOptions {
                    db_path: "mydatabase".to_string(),
                    cluster_domain: "mycluster.test".to_string(),
                    http_port: 80,
                    https_options: None,
                }),
                agent_options: None,
                cert_options: None,
            },
            opts
        );
    }

    #[test]
    #[should_panic(expected = "Expected ")]
    fn test_proxy_no_cluster_domain() {
        parse_args(&["--db-path", "mydatabase"]).unwrap();
    }

    #[test]
    #[should_panic(expected = "Expected ")]
    fn test_proxy_no_db_path() {
        parse_args(&["--cluster-domain", "blah"]).unwrap();
    }

    #[test]
    #[should_panic(expected = "Expected ")]
    fn test_migrate_no_db_path() {
        parse_args(&["migrate"]).unwrap();
    }

    #[test]
    #[should_panic(expected = "Expected --https-certificate")]
    fn test_key_but_no_cert() {
        parse_args(&[
            "--db-path",
            "mydatabase",
            "--cluster-domain",
            "mycluster.test",
            "--https-private-key",
            "mycert.key",
        ])
        .unwrap();
    }

    #[test]
    #[should_panic(expected = "Expected --https-private-key")]
    fn test_cert_but_no_key() {
        parse_args(&[
            "--db-path",
            "mydatabase",
            "--cluster-domain",
            "mycluster.test",
            "--https-certificate",
            "mycert.cert",
        ])
        .unwrap();
    }

    #[test]
    fn test_proxy_with_https() {
        let opts = parse_args(&[
            "--db-path",
            "mydatabase",
            "--cluster-domain",
            "mycluster.test",
            "--https-certificate",
            "mycert.cert",
            "--https-private-key",
            "mycert.key",
            "--ip",
            "123.123.123.123",
            "--host-ip",
            "56.56.56.56",
            "--nats-url",
            "nats://foo@bar",
            "serve",
            "--proxy",
            "--agent",
        ])
        .unwrap();
        assert_eq!(
            DronePlan::RunService {
                proxy_options: Some(ProxyOptions {
                    db_path: "mydatabase".to_string(),
                    cluster_domain: "mycluster.test".to_string(),
                    http_port: 80,
                    https_options: Some(ProxyHttpsOptions {
                        key_paths: KeyCertPathPair {
                            private_key_path: PathBuf::from("mycert.key"),
                            certificate_path: PathBuf::from("mycert.cert"),
                        },
                        port: 443
                    }),
                }),
                agent_options: Some(AgentOptions {
                    db_path: "mydatabase".to_string(),
                    cluster_domain: "mycluster.test".to_string(),
                    docker_options: DockerOptions {
                        transport: DockerApiTransport::Socket("/var/run/docker.sock".to_string()),
                        runtime: None,
                    },
                    ip: IpProvider::Literal("123.123.123.123".parse().unwrap()),
                    host_ip: "56.56.56.56".parse().unwrap(),
                    nats_url: "nats://foo@bar".to_string(),
                }),
                cert_options: None,
            },
            opts
        );
    }

    #[test]
    fn test_proxy_with_ports() {
        let opts = parse_args(&[
            "--db-path",
            "mydatabase",
            "--cluster-domain",
            "mycluster.test",
            "--https-certificate",
            "mycert.cert",
            "--https-private-key",
            "mycert.key",
            "--http-port",
            "12345",
            "--https-port",
            "12398",
            "--ip",
            "123.123.123.123",
            "--host-ip",
            "56.56.56.56",
            "--nats-url",
            "nats://foo@bar",
            "--acme-server",
            "https://acme-server",
        ])
        .unwrap();
        assert_eq!(
            DronePlan::RunService {
                proxy_options: Some(ProxyOptions {
                    db_path: "mydatabase".to_string(),
                    cluster_domain: "mycluster.test".to_string(),
                    http_port: 12345,
                    https_options: Some(ProxyHttpsOptions {
                        key_paths: KeyCertPathPair {
                            private_key_path: PathBuf::from("mycert.key"),
                            certificate_path: PathBuf::from("mycert.cert"),
                        },
                        port: 12398
                    }),
                }),
                agent_options: Some(AgentOptions {
                    db_path: "mydatabase".to_string(),
                    cluster_domain: "mycluster.test".to_string(),
                    docker_options: DockerOptions {
                        transport: DockerApiTransport::Socket("/var/run/docker.sock".to_string()),
                        runtime: None,
                    },
                    ip: IpProvider::Literal("123.123.123.123".parse().unwrap()),
                    host_ip: "56.56.56.56".parse().unwrap(),
                    nats_url: "nats://foo@bar".to_string(),
                }),
                cert_options: Some(CertOptions {
                    acme_server_url: "https://acme-server".to_string(),
                    cluster_domain: "mycluster.test".to_string(),
                    key_paths: KeyCertPathPair {
                        private_key_path: PathBuf::from("mycert.key"),
                        certificate_path: PathBuf::from("mycert.cert"),
                    },
                    nats_url: "nats://foo@bar".to_string(),
                })
            },
            opts
        );
    }
}
