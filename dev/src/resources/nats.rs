use crate::container::{ContainerResource, ContainerSpec};
use crate::scratch_dir;
use crate::util::wait_for_port;
use anyhow::{anyhow, Result};
use chrono::Utc;
use plane_core::nats::TypedNats;
use plane_core::nats_connection::{NatsAuthorization, NatsConnectionSpec};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::net::SocketAddrV4;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

const NATS_USER: &str = "myuser";
const NATS_PASS: &str = "mypass";

pub struct Nats {
    container: ContainerResource,
    #[allow(unused)]
    log_handle: JoinHandle<()>,
}

impl Nats {
    pub fn connection_spec(&self) -> NatsConnectionSpec {
        NatsConnectionSpec {
            auth: Some(NatsAuthorization::UserAndPassword {
                username: NATS_USER.into(),
                password: NATS_PASS.into(),
            }),
            hosts: vec![self.container.ip.to_string()],
        }
    }

    pub async fn connection(&self) -> Result<TypedNats> {
        let nc = self.connection_spec().connect("test.inbox").await?;
        nc.initialize_jetstreams().await?;
        Ok(nc)
    }

    pub async fn new() -> Result<Nats> {
        let spec = ContainerSpec {
            name: "nats".into(),
            image: "docker.io/nats:2.8".into(),
            environment: HashMap::new(),
            command: vec![
                "--jetstream".into(),
                "--user".into(),
                NATS_USER.into(),
                "--pass".into(),
                NATS_PASS.into(),
            ],
            volumes: Vec::new(),
        };

        let container = ContainerResource::new(&spec).await?;

        wait_for_port(SocketAddrV4::new(container.ip, 4222), 10_000).await?;

        let conn =
            async_nats::ConnectOptions::with_user_and_password(NATS_USER.into(), NATS_PASS.into())
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
}
