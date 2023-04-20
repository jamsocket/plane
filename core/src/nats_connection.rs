use crate::{nats::TypedNats, retry::do_with_retry};
use anyhow::Result;
use async_nats::{ConnectOptions, ServerAddr};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

/// This matches NATS' Authorization struct, which is crate-private.
/// https://github.com/nats-io/nats.rs/blob/2f53feab2eac4c01fb470309a3af2c9920f9224a/async-nats/src/lib.rs#L1249
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum NatsAuthorization {
    /// Authenticate using a token.
    Token { token: String },

    /// Authenticate using a username and password.
    UserAndPassword { username: String, password: String },
    // TODO: JWT
}

#[derive(Serialize, Deserialize)]
pub struct NatsConnectionSpec {
    pub auth: Option<NatsAuthorization>,
    pub hosts: Vec<String>,
}

impl NatsConnectionSpec {
    pub fn from_url(url: &str) -> Result<Self> {
        let url = Url::parse(url)?;

        let auth = if let Some(password) = url.password().as_ref() {
            Some(NatsAuthorization::UserAndPassword {
                username: url.username().to_string(),
                password: (*password).to_string(),
            })
        } else if !url.username().is_empty() {
            Some(NatsAuthorization::Token {
                token: url.username().to_string(),
            })
        } else {
            None
        };

        let hosts = vec![url.host_str().unwrap_or("localhost").into()];

        Ok(NatsConnectionSpec { auth, hosts })
    }

    pub fn connect_options(&self, inbox_prefix: &str) -> ConnectOptions {
        let mut opts = match &self.auth {
            None => ConnectOptions::default(),
            Some(NatsAuthorization::Token { token }) => ConnectOptions::with_token(token.into()),
            Some(NatsAuthorization::UserAndPassword { username, password }) => {
                ConnectOptions::with_user_and_password(username.into(), password.into())
            }
        };

        opts = opts.custom_inbox_prefix(inbox_prefix);
        opts
    }

    pub async fn connect_with_retry(&self, inbox_prefix: &str) -> Result<TypedNats> {
        let server_addrs: Result<Vec<ServerAddr>, _> =
            self.hosts.iter().map(|d| ServerAddr::from_str(d)).collect();
        let server_addrs = server_addrs?;

        let nats = do_with_retry(
            || {
                async_nats::connect_with_options(
                    &server_addrs as &[ServerAddr],
                    self.connect_options(inbox_prefix),
                )
            },
            30,
            Duration::from_secs(10),
        )
        .await?;

        TypedNats::new(nats).await
    }

    pub async fn connect(&self, inbox_prefix: &str) -> Result<TypedNats> {
        let server_addrs: Result<Vec<ServerAddr>, _> =
            self.hosts.iter().map(|d| ServerAddr::from_str(d)).collect();
        let server_addrs = server_addrs?;

        let nats = async_nats::connect_with_options(
            &server_addrs as &[ServerAddr],
            self.connect_options(inbox_prefix),
        )
        .await?;

        TypedNats::new(nats).await
    }
}
