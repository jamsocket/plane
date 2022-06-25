use crate::keys::load_certs;
use anyhow::{anyhow, Result, Context};
use reqwest::Client;
use std::{env::var, path::PathBuf};

pub fn get_https_client() -> Result<Client> {
    if let Ok(value) = var("SPAWNER_TEST_ALLOWED_CERTIFICATE") {
        tracing::warn!(cert_file=%value, "Using overridden certificate for remote ACME server. This should only be used in tests.");

        let cert = load_certs(&PathBuf::from(value)).context("Error loading override certificate.")?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Certificate file contained no certificates."))?;

        let cert = reqwest::Certificate::from_der(&cert.0).context("Error converting override certificate.")?;

        Ok(reqwest::Client::builder()
            .add_root_certificate(cert)
            .build().context("Error building HTTPS client.")?)
    } else {
        Ok(reqwest::Client::new())
    }
}
