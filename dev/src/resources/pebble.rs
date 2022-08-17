use super::{certs::Certificates, tempdir::TemporaryDirectory};
use crate::container::{ContainerResource, ContainerSpec};
use anyhow::Result;
use serde_json::json;

pub struct PebbleService {
    container: ContainerResource,
    certs: Certificates,
    config_dir: TemporaryDirectory,
}

impl PebbleService {
    pub fn directory_url(&self) -> String {
        todo!()
    }
}

pub async fn pebble() -> Result<PebbleService> {
    let certs = Certificates::new(vec!["localhost".to_string()])?;
    let config_dir = TemporaryDirectory::new()?;

    let pebble_config = json!({
        "pebble": {
            "listenAddress": "0.0.0.0:443",
            "managementListenAddress": "0.0.0.0:15000",
            "certificate": "/etc/auth/local-cert.pem",
            "privateKey": "/etc/auth/local-cert.key",
            "httpPort": 5002,
            "tlsPort": 5001,
            "ocspResponderURL": "",
        }
    });

    std::fs::write(
        config_dir.path.join("config.json"),
        serde_json::to_string(&pebble_config)?,
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
            (config_dir.path(), "/etc/pebble".into()),
            (certs.path(), "/etc/auth".into()),
        ],
    };

    Ok(PebbleService {
        container: ContainerResource::new(&spec).await?,
        certs,
        config_dir,
    })
}
