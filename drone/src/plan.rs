use super::{agent::AgentOptions, cert::CertOptions, proxy::ProxyOptions};
use crate::config::DroneConfig;
use crate::database::DroneDatabase;
use anyhow::Result;
use plane_core::{
    nats::TypedNats,
    types::{ClusterName, DroneId},
};

pub struct DronePlan {
    pub proxy_options: Option<ProxyOptions>,
    pub agent_options: Option<AgentOptions>,
    pub cert_options: Option<CertOptions>,
    pub nats: Option<TypedNats>,
    pub drone_id: DroneId,
}

impl DronePlan {
    pub async fn from_drone_config(config: DroneConfig) -> Result<Self> {
        let drone_id = config.drone_id.unwrap_or_else(DroneId::new_random);
        tracing::info!(?drone_id, "Starting drone.");

        let cluster: ClusterName = ClusterName::new(&config.cluster_domain);
        let inbox_prefix = format!("cluster.{}.inbox", cluster.subject_name());

        let nats = if let Some(nats) = config.nats {
            Some(nats.connect_with_retry(&inbox_prefix).await?)
        } else {
            None
        };

        let db = DroneDatabase::new(&config.db_path).await?;

        let cert_options = if let Some(acme_config) = config.acme {
            Some(CertOptions {
                acme_server_url: acme_config.server,
                email: acme_config.admin_email,
                cluster_domain: config.cluster_domain.clone(),
                key_paths: config
                    .cert
                    .clone()
                    .expect("Expected cert path configuration if ACME is provided."),
                nats: nats.clone().expect("Expected --nats-url."),
                acme_eab_keypair: acme_config.eab,
            })
        } else {
            None
        };

        let proxy_options = if let Some(proxy_config) = config.proxy {
            let (bind_port, bind_redir_port, key_pair) =
                match (config.cert, proxy_config.https_port) {
                    (key_pair @ Some(_), Some(https_port)) => {
                        (https_port, Some(proxy_config.http_port), key_pair)
                    }
                    (key_pair @ Some(_), None) => (443, Some(proxy_config.http_port), key_pair),
                    (None, Some(_)) => panic!("may not specify https_port without cert"),
                    (None, None) => (proxy_config.http_port, None, None),
                };
            Some(ProxyOptions {
                cluster_domain: config.cluster_domain.clone(),
                db: db.clone(),
                bind_ip: proxy_config.bind_ip,
                bind_redir_port,
                bind_port,
                key_pair,
                passthrough: proxy_config.passthrough,
                allow_path_routing: proxy_config.allow_path_routing,
            })
        } else {
            None
        };

        let agent_options = if let Some(agent_config) = config.agent {
            Some(AgentOptions {
                cluster_domain: ClusterName::new(&config.cluster_domain),
                drone_id: drone_id.clone(),
                db,
                docker_options: agent_config.docker,
                nats: nats
                    .clone()
                    .expect("Expected --nats-url for running agent."),
                ip: agent_config.ip,
            })
        } else {
            None
        };

        Ok(DronePlan {
            agent_options,
            cert_options,
            nats,
            drone_id,
            proxy_options,
        })
    }
}
