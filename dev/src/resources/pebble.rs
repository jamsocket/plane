use std::collections::HashMap;

use super::certs::SelfSignedCert;
use crate::{
    container::{ContainerResource, ContainerSpec},
    scratch_dir,
    util::wait_for_url,
};
use anyhow::{Context, Result};
use plane_drone::cert::acme::AcmeEabConfiguration;
use reqwest::Client;
use serde_json::json;

pub struct Pebble {
    container: ContainerResource,
    certs: SelfSignedCert,
}

impl Pebble {
    pub fn directory_url(&self) -> String {
        format!("https://{}/dir", self.container.ip)
    }

    pub fn client(&self) -> Result<Client> {
        let cert = reqwest::Certificate::from_pem(self.certs.cert_pem.as_bytes())
            .context("Parsing certificate")?;

        Ok(reqwest::Client::builder()
            .add_root_certificate(cert)
            .danger_accept_invalid_hostnames(true)
            .build()?)
    }

    pub async fn new() -> Result<Pebble> {
        Self::new_impl(None).await
    }

    pub async fn new_eab(eab_keypair: &AcmeEabConfiguration) -> Result<Pebble> {
        Self::new_impl(Some(eab_keypair)).await
    }

    async fn new_impl(eab_keypair: Option<&AcmeEabConfiguration>) -> Result<Pebble> {
        let certs = SelfSignedCert::new("pebble-certs", vec!["localhost".to_string()])?;
        let config_dir = scratch_dir("pebble-config");

        let external_account_mac_keys: HashMap<String, String> = eab_keypair
            .iter()
            .map(|keypair| (keypair.key_id.clone(), keypair.eab_key_b64()))
            .collect();
        let external_account_binding_required = !external_account_mac_keys.is_empty();

        let pebble_config = json!({
            "pebble": {
                "listenAddress": "0.0.0.0:443",
                "managementListenAddress": "0.0.0.0:15000",
                "certificate": "/etc/auth/selfsigned.pem",
                "privateKey": "/etc/auth/selfsigned.key",
                "httpPort": 5002,
                "tlsPort": 5001,
                "ocspResponderURL": "",
                "externalAccountMacKeys": external_account_mac_keys,
                "externalAccountBindingRequired": external_account_binding_required,
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

        let pebble = Pebble {
            container: ContainerResource::new(&spec).await?,
            certs,
        };

        wait_for_url(&pebble.directory_url(), 5_000).await?;

        Ok(pebble)
    }
}
