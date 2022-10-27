use crate::container::{ContainerResource, ContainerSpec};
use crate::util::wait_for_port;
use crate::{scratch_dir, LOG_TO_STDOUT};
use anyhow::{anyhow, Result};
use chrono::Utc;
use plane_core::nats::TypedNats;
use plane_core::nats_connection::{NatsAuthorization, NatsConnectionSpec};
use rand::Rng;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::net::SocketAddrV4;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

const NATS_TOKEN: &str = "mytoken";
pub struct Nats {
    pub container: ContainerResource,
    #[allow(unused)]
    log_handle: JoinHandle<()>,
}

impl Nats {
    pub fn connection_spec(&self) -> NatsConnectionSpec {
        NatsConnectionSpec {
            auth: Some(NatsAuthorization::Token {
                token: NATS_TOKEN.into(),
            }),
            hosts: vec![self.container.ip.to_string()],
        }
    }

    pub async fn connection(&self) -> Result<TypedNats> {
        self.connection_spec().connect().await
    }

    pub async fn new(persist: bool) -> Result<Nats> {
        let spec = ContainerSpec {
            name: "nats".into(),
            image: "docker.io/nats:2.8".into(),
            environment: HashMap::new(),
            command: vec!["--jetstream".into(), "--auth".into(), NATS_TOKEN.into()],
            volumes: match persist {
                false => Vec::new(),
                true => vec![(
                    format!("/tmp/nats/jetstream-{}", rand::thread_rng().gen::<u16>()),
                    "/tmp/nats/jetstream".into(),
                )],
            },
        };

        let container = ContainerResource::new(&spec).await?;

        wait_for_port(SocketAddrV4::new(container.ip, 4222), 10_000).await?;

        let conn = async_nats::ConnectOptions::with_token(NATS_TOKEN.into())
            .connect(container.ip.to_string())
            .await?;

        let mut output = File::create(scratch_dir("logs").join("nats-wiretap.txt"))?;
        let mut wiretap = conn
            .subscribe(">".into())
            .await
            .map_err(|_| anyhow!("Couldn't subscribe to NATS wiretap."))?;

        let log_handle = tokio::spawn(async move {
            while let Some(v) = wiretap.next().await {
                let message = std::str::from_utf8(&v.payload).unwrap();
                if LOG_TO_STDOUT {
                    std::io::stdout().write_all(&v.payload).unwrap();
                    std::io::stdout().write_all(b"\n").unwrap();
                }
                output
                    .write_fmt(format_args!(
                        "{} {} {:?}: {}\n",
                        Utc::now().to_rfc3339(),
                        v.subject,
                        v.reply,
                        message
                    ))
                    .unwrap();
            }
        });

        Ok(Nats {
            container,
            log_handle,
        })
    }

    pub async fn restart(&mut self) -> Result<()> {
        self.container = self.container.restart().await?;
        Ok(())
    }
}
