use crate::common::async_drop::AsyncDrop;
use crate::common::docker::Container;
use crate::common::test_env::TestEnvironment;
use anyhow::anyhow;
use anyhow::Result;
use bollard::{container::Config, Docker};
use plane::proxy::AcmeEabConfiguration;
use reqwest::Client;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};
use url::Url;

const POLL_LOOP_SLEEP: u64 = 100;
const PEBBLE_IMAGE: &str = "docker.io/letsencrypt/pebble:latest";

fn get_start_script(dns_port: u16) -> String {
    format!(
        r#"#!/bin/sh

set -e

DNS_PORT={}
DNS_IP=$(getent hosts host.docker.internal | awk '{{ print $1 }}')
DNS_SERVER=$DNS_IP:$DNS_PORT

echo "Starting pebble with DNS server $DNS_SERVER"

/usr/bin/pebble -config /etc/pebble/config.json -dnsserver $DNS_SERVER
"#,
        dns_port
    )
}

pub struct Pebble {
    container: Container,
    #[allow(dead_code)] // Used in tests
    pub directory_url: Url,
}

impl Pebble {
    async fn directory_url(container: &Container) -> Url {
        let v = format!(
            "https://127.0.0.1:{}/dir",
            container.get_port(14000).await.unwrap()
        );

        Url::parse(&v).unwrap()
    }

    pub fn client() -> Result<Client> {
        Ok(reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?)
    }

    pub async fn wait_for_url(url: &Url, timeout_seconds: u64) -> Result<()> {
        let initial_time = SystemTime::now();
        let client = Self::client()?;

        loop {
            let result = client
                .get(url.clone())
                .timeout(Duration::from_secs(1))
                .send()
                .await;

            match result {
                Ok(_) => return Ok(()),
                Err(e) => {
                    if SystemTime::now()
                        .duration_since(initial_time)
                        .unwrap()
                        .as_secs()
                        > timeout_seconds
                    {
                        return Err(anyhow!(
                            "Failed to load URL {} after {} seconds. Last error was {:?}",
                            url,
                            timeout_seconds,
                            e
                        ));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(POLL_LOOP_SLEEP)).await;
        }
    }

    pub async fn new(
        env: &TestEnvironment,
        dns_port: u16,
        eab_keypair: Option<AcmeEabConfiguration>,
    ) -> Result<Pebble> {
        let scratch_dir = env.scratch_dir.clone();

        #[cfg(target_os = "macos")]
        avoid_weird_mac_bug(&env.run_name, &scratch_dir).await?;

        let pebble_dir = scratch_dir.canonicalize()?.join("pebble");
        std::fs::create_dir_all(&pebble_dir)?;

        let mut pebble_config = json!({
            "pebble": {
                "listenAddress": "0.0.0.0:14000",
                "managementListenAddress": "0.0.0.0:15000",
                "certificate": "test/certs/localhost/cert.pem",
                "privateKey": "test/certs/localhost/key.pem",
                "httpPort": 5002,
                "tlsPort": 5001,
                "ocspResponderURL": "",
                "externalAccountBindingRequired": false,
                "domainBlocklist": ["blocked-domain.example"],
                "retryAfter": {
                    "authz": 3,
                    "order": 5
                },
                "certificateValidityPeriod": 157766400,
            }
        });

        if let Some(eab_keypair) = eab_keypair {
            pebble_config["pebble"]["externalAccountBindingRequired"] = json!(true);
            pebble_config["pebble"]["externalAccountMacKeys"] = json!({
                eab_keypair.clone().key_id : eab_keypair.eab_key_b64()
            });
        }

        std::fs::write(
            pebble_dir.join("config.json"),
            serde_json::to_string_pretty(&pebble_config)?,
        )?;

        std::fs::write(pebble_dir.join("start.sh"), get_start_script(dns_port))?;

        std::fs::set_permissions(
            pebble_dir.join("start.sh"),
            std::fs::Permissions::from_mode(0o755),
        )?;

        let config = Config {
            image: Some(PEBBLE_IMAGE.to_string()),
            cmd: Some(vec!["/etc/pebble/start.sh".to_string()]),
            env: Some(vec![
                // https://github.com/letsencrypt/pebble?tab=readme-ov-file#testing-at-full-speed
                "PEBBLE_VA_NOSLEEP=1".to_string(),
            ]),
            host_config: Some(bollard::service::HostConfig {
                binds: Some(vec![format!(
                    "{}:/etc/pebble",
                    pebble_dir.to_str().unwrap()
                )]),
                port_bindings: Some(
                    vec![(
                        "14000/tcp".to_string(),
                        Some(vec![bollard::service::PortBinding {
                            host_ip: Some("0.0.0.0".to_string()),
                            host_port: None,
                        }]),
                    )]
                    .into_iter()
                    .collect(),
                ),
                extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let docker = Docker::connect_with_local_defaults()?;
        let name = format!("pebble-{}", env.run_name);
        let container =
            Container::create(name, docker, config, Some(scratch_dir.to_owned())).await?;
        let directory_url = Self::directory_url(&container).await;

        Self::wait_for_url(&directory_url, 5).await?;

        let pebble = Pebble {
            container,
            directory_url,
        };

        Ok(pebble)
    }
}

/// For some reason, Docker on Mac (Sequoia) will sometimes 500 when mounting the pebble directory
/// inside the scratch directory. For whatever reason, first starting a dummy container that mounts
/// the scratch directory itself (i.e. the parent of the pebble directory) seems to prevent this
/// from happening.
#[cfg(target_os = "macos")]
pub async fn avoid_weird_mac_bug(name: &str, scratch_dir: &Path) -> Result<()> {
    println!(
        "Creating dummy container for macos {}",
        scratch_dir.to_str().unwrap()
    );
    let docker = Docker::connect_with_local_defaults()?;
    let name = format!("dummy-{}", name);

    let config = Config {
        image: Some("alpine:latest".to_string()), // not pinning because the actual container doesn't matter
        host_config: Some(bollard::service::HostConfig {
            binds: Some(vec![format!("{}:/mount", scratch_dir.to_str().unwrap())]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let container = Container::create(name, docker, config, Some(scratch_dir.to_owned())).await?;
    container.stop().await?;

    Ok(())
}

#[async_trait::async_trait]
impl AsyncDrop for Pebble {
    async fn drop_future(&self) -> Result<()> {
        tracing::info!("Stopping pebble.");
        self.container.stop().await?;
        Ok(())
    }
}
