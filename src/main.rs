use anyhow::Result;
use clap::Parser;
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::PathBuf;

mod proxy;

#[derive(Parser)]
struct Opts {
    /// Path to sqlite3 database file to use for getting route information.
    ///
    /// This may be a file that does not exist. In this case, it will be created.
    #[clap(long)]
    db_path: PathBuf,

    /// Port to listen for HTTP requests on.
    #[clap(long)]
    http_port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    let co = SqliteConnectOptions::new()
        .filename(opts.db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(co).await?;

    migrate!("./migrations").run(&pool).await?;

    if let Some(http_port) = opts.http_port {
        proxy::serve(http_port).await?;
    }

    Ok(())
}
