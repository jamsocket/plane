use crate::{
    names::{OrRandom, ProxyName},
    proxy::{AcmeConfig, AcmeEabConfiguration, ServerPortConfig},
    types::ClusterName,
};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;
use url::Url;

use super::ProxyConfig;

const LOCAL_HTTP_PORT: u16 = 9090;
const PROD_HTTP_PORT: u16 = 80;
const PROD_HTTPS_PORT: u16 = 443;

#[derive(Parser)]
pub struct ProxyOpts {
    #[clap(long)]
    name: Option<ProxyName>,

    #[clap(long)]
    controller_url: Url,

    #[clap(long)]
    cluster: ClusterName,

    #[clap(long)]
    https: bool,

    #[clap(long)]
    http_port: Option<u16>,

    #[clap(long)]
    https_port: Option<u16>,

    #[clap(long)]
    cert_path: Option<PathBuf>,

    #[clap(long)]
    acme_endpoint: Option<Url>,

    #[clap(long)]
    acme_email: Option<String>,

    /// Key identifier when using ACME External Account Binding.
    #[clap(long)]
    acme_eab_kid: Option<String>,

    /// HMAC key when using ACME External Account Binding.
    #[clap(long)]
    acme_eab_hmac_key: Option<String>,

    /// URL to redirect the root path to.
    #[clap(long)]
    root_redirect_url: Option<Url>,
}

impl ProxyOpts {
    pub fn into_config(self) -> Result<ProxyConfig> {
        let name = self.name.or_random();

        let port_config = match (self.https, self.http_port, self.https_port) {
            (false, None, None) => ServerPortConfig {
                http_port: LOCAL_HTTP_PORT,
                https_port: None,
            },
            (true, None, None) => ServerPortConfig {
                http_port: PROD_HTTP_PORT,
                https_port: Some(PROD_HTTPS_PORT),
            },
            (true, Some(http_port), None) => ServerPortConfig {
                http_port,
                https_port: Some(PROD_HTTPS_PORT),
            },
            (_, None, Some(https_port)) => ServerPortConfig {
                http_port: PROD_HTTP_PORT,
                https_port: Some(https_port),
            },
            (_, Some(http_port), https_port) => ServerPortConfig {
                http_port,
                https_port,
            },
        };

        let acme_eab_keypair = match (self.acme_eab_hmac_key, self.acme_eab_kid) {
            (Some(hmac_key), Some(kid)) => Some(AcmeEabConfiguration::new(&kid, &hmac_key)?),
            (None, Some(_)) | (Some(_), None) => {
                return Err(anyhow!(
                    "Must specify both --acme-eab-hmac-key and --acme-eab-kid or neither."
                ))
            }
            _ => None,
        };

        let acme_config = match (self.acme_endpoint, self.acme_email) {
            (Some(_), None) => {
                return Err(anyhow!(
                    "Must specify --acme-email when using --acme-endpoint."
                ));
            }
            (None, Some(_)) => {
                return Err(anyhow!(
                    "Must specify --acme-endpoint when using --acme-email."
                ));
            }
            (Some(endpoint), Some(email)) => Some(AcmeConfig {
                endpoint,
                mailto_email: email,
                acme_eab_keypair,
                accept_insecure_certs_for_testing: false,
            }),
            (None, None) => None,
        };

        Ok(ProxyConfig {
            name,
            controller_url: self.controller_url,
            cluster: self.cluster,
            cert_path: self.cert_path,
            port_config,
            acme_config,
            root_redirect_url: self.root_redirect_url,
        })
    }
}
