use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::PathBuf;
use std::str::FromStr;

use crate::conf;

#[derive(Clone)]
pub struct Database(SqlitePool);

impl Database {
    pub fn new(pool: SqlitePool) -> Self {
        Self(pool)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }
}

pub async fn init_db() -> Result<SqlitePool> {
    let db_path = get_db_path()?;

    // Ensure the parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
        log::debug!("Created database directory: {}", parent.display());
    }

    log::info!("Initializing database at: {}", db_path.display());

    // Create connection options with create_if_missing
    let db_url = format!("sqlite://{}", db_path.display());
    let connect_options = SqliteConnectOptions::from_str(&db_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    // Run migrations
    run_migrations(&pool).await?;

    log::info!("Database initialized successfully");
    Ok(pool)
}

pub fn get_db_path() -> Result<PathBuf> {
    Ok(conf::get_project_dirs()?.data_dir().join("dictate.db"))
}

async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    log::debug!("Running database migrations");
    sqlx::migrate!("./migrations").run(pool).await?;
    log::debug!("Migrations completed successfully");
    Ok(())
}
