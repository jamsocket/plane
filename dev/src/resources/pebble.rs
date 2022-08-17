use super::certs::Certificates;
use crate::{
    container::{ContainerResource, ContainerSpec},
    scratch_dir,
    util::wait_for_url,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::json;

pub struct PebbleService {
    container: ContainerResource,
    certs: Certificates,
}

impl PebbleService {
    pub fn directory_url(&self) -> String {
        format!("https://{}/dir", self.container.ip)
    }

    pub fn client(&self) -> Result<Client> {
        let cert = reqwest::Certificate::from_pem(&self.certs.cert_pem.as_bytes())
            .context("Parsing certificate")?;

        Ok(reqwest::Client::builder()
            .add_root_certificate(cert)
            .danger_accept_invalid_hostnames(true)
            .build()?)
    }
}

pub async fn pebble() -> Result<PebbleService> {
    let certs = Certificates::new("pebble-certs", vec!["localhost".to_string()])?;
    let config_dir = scratch_dir("pebble-config");

    let pebble_config = json!({
        "pebble": {
            "listenAddress": "0.0.0.0:443",
            "managementListenAddress": "0.0.0.0:15000",
            "certificate": "/etc/auth/selfsigned.pem",
            "privateKey": "/etc/auth/selfsigned.key",
            "httpPort": 5002,
            "tlsPort": 5001,
            "ocspResponderURL": "",
        }
    });

    std::fs::write(
        config_dir.join("config.json"),
        serde_json::to_string_pretty(&pebble_config)?,
    )?;

    let spec = ContainerSpec {
        name: "pebble".into(),
        image: "docker.io/letsencrypt/pebble:latest".into(),
        environment: vec![("PEBBLE_VA_ALWAYS_VALID".into(), "1".into())]
            .into_iter()
            .collect(),
        command: vec![
            "/usr/bin/pebble".into(),
            "-config".into(),
            "/etc/pebble/config.json".into(),
        ],
        volumes: vec![
            (config_dir.to_str().unwrap().into(), "/etc/pebble".into()),
            (certs.path(), "/etc/auth".into()),
        ],
    };

    let pebble = PebbleService {
        container: ContainerResource::new(&spec).await?,
        certs,
    };

    wait_for_url(&pebble.directory_url(), 5_000).await?;

    Ok(pebble)
}
