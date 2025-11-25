use anyhow::Result;
use directories::ProjectDirs;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::PathBuf;
use std::str::FromStr;

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
        eprintln!("[db] Created database directory: {}", parent.display());
    }

    eprintln!("[db] Initializing database at: {}", db_path.display());

    // Create connection options with create_if_missing
    let db_url = format!("sqlite://{}", db_path.display());
    let connect_options = SqliteConnectOptions::from_str(&db_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    // Run migrations
    run_migrations(&pool).await?;

    eprintln!("[db] Database initialized successfully");
    Ok(pool)
}

pub fn get_db_path() -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
        .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?;

    let data_dir = project_dirs.data_dir();
    Ok(data_dir.join("dictate.db"))
}

async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    eprintln!("[db] Running database migrations");
    sqlx::migrate!("./migrations").run(pool).await?;
    eprintln!("[db] Migrations completed successfully");
    Ok(())
}
