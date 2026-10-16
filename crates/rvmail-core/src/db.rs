//! Conexión a PostgreSQL.

use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub async fn connect(url: &str, max: u32) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .context("No se pudo conectar a PostgreSQL")
}
