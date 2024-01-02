use crate::common::{async_drop::AsyncDrop, docker::Container, test_env::TestEnvironment};
use anyhow::{Context, Result};
use bollard::{container::Config, Docker};
use plane::database::{connect_and_migrate, PlaneDatabase};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

const POSTGRES_IMAGE: &str = "postgres:16";

pub struct DevDatabase {
    pub db: PlaneDatabase,
    docker: Docker,
    container: Container,
    scratch_dir: PathBuf,
    stopped: OnceLock<()>,
    run_name: String,
    port: u16,
}

fn container_config() -> Config<String> {
    Config {
        image: Some(POSTGRES_IMAGE.to_string()),
        env: Some(vec!["POSTGRES_HOST_AUTH_METHOD=trust".to_string()]),
        host_config: Some(bollard::service::HostConfig {
            port_bindings: Some(
                vec![(
                    "5432/tcp".to_string(),
                    Some(vec![bollard::service::PortBinding {
                        host_ip: Some("0.0.0.0".to_string()),
                        host_port: None,
                    }]),
                )]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn pg_dump_container_config(port: u16) -> Config<String> {
    Config {
        image: Some(POSTGRES_IMAGE.to_string()),
        env: Some(vec!["POSTGRES_HOST_AUTH_METHOD=trust".to_string()]),
        cmd: Some(vec![
            "pg_dump".to_string(),
            "-h".to_string(),
            "host.docker.internal".to_string(),
            "-p".to_string(),
            port.to_string(),
            "-U".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        ]),
        host_config: Some(bollard::service::HostConfig {
            extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn attempt_to_connect(connection_string: &str) -> Result<PlaneDatabase> {
    for _ in 0..30 {
        match connect_and_migrate(&connection_string).await {
            Ok(db) => return Ok(db),
            Err(e) => {
                tracing::info!(?e, "Failed to connect and migrate, retrying...");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    Err(anyhow::anyhow!("Failed to connect and migrate."))
}

impl DevDatabase {
    pub async fn start(env: &TestEnvironment) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()?;
        let container = Container::create(
            format!("postgres-{}", env.run_name),
            docker.clone(),
            container_config(),
            Some(env.scratch_dir.clone()),
        )
        .await?;

        let port = container.get_port(5432).await?;
        let connection_string = Self::get_connection_string(port);

        println!("Connection_string: {}", connection_string);

        let db = attempt_to_connect(&connection_string).await?;

        Ok(Self {
            docker,
            container,
            scratch_dir: env.scratch_dir.clone(),
            stopped: OnceLock::new(),
            db,
            run_name: env.run_name.clone(),
            port,
        })
    }

    fn get_connection_string(port: u16) -> String {
        format!("postgres://postgres@127.0.0.1:{port}/postgres")
    }

    async fn dump_db(&self, path: &Path) -> Result<()> {
        let config = pg_dump_container_config(self.port);
        tracing::info!(?config, "Dumping database...");
        let name = format!("pg_dump-{}", self.run_name);
        let container = Container::create(name, self.docker.clone(), config, None).await?;
        tracing::info!("Waiting for dump to complete...");

        let result = container
            .collect_stdout()
            .await
            .context("Error dumping database")?;

        container.check_exit_code().await?;

        std::fs::write(path, result)?;

        tracing::info!("Dump complete.");
        Ok(())
    }
}

#[async_trait::async_trait]
impl AsyncDrop for DevDatabase {
    async fn drop_future(&self) -> Result<()> {
        tracing::info!("Stopping database...");
        self.dump_db(&self.scratch_dir.join("db_dump.sql")).await?;
        self.container.stop().await?;
        let _ = self.stopped.set(());
        Ok(())
    }
}
