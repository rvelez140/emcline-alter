//! Configuración por variables de entorno (regla 7 del proyecto).

use crate::crypto::MasterKey;
use anyhow::{Context, Result};
use std::time::Duration;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn required(name: &str) -> Result<String> {
    var(name).with_context(|| format!("Falta la variable de entorno {name}"))
}

fn num<T: std::str::FromStr>(name: &str, default: T) -> T {
    var(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn flag(name: &str) -> bool {
    var(name).is_some_and(|v| v == "true" || v == "1")
}

/// Ajustes comunes a todos los procesos.
#[derive(Clone)]
pub struct CoreConfig {
    pub database_url: String,
    pub redis_url: String,
    pub master_key: MasterKey,
    /// Acepta certificados autofirmados en IMAP/SMTP (servidor propio sin Let's Encrypt)
    pub allow_self_signed: bool,
}

impl CoreConfig {
    pub fn from_env() -> Result<Self> {
        let master_key =
            MasterKey::from_base64(&required("MASTER_KEY")?).context("MASTER_KEY no es válida")?;
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            redis_url: var("REDIS_URL").unwrap_or_else(|| "redis://localhost:6379".into()),
            master_key,
            allow_self_signed: flag("IMAP_ALLOW_SELF_SIGNED"),
        })
    }
}

/// Ajustes del sincronizador.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub sync_days: i64,
    pub sync_interval: Duration,
    pub batch_size: usize,
    pub max_accounts: usize,
    pub heartbeat_file: String,
    pub db_pool_size: u32,
}

impl WorkerConfig {
    pub fn from_env() -> Self {
        Self {
            sync_days: num("SYNC_DAYS", 90),
            sync_interval: Duration::from_secs(num("SYNC_INTERVAL_SEC", 300)),
            batch_size: num("SYNC_BATCH_SIZE", 100),
            max_accounts: num("WORKER_MAX_ACCOUNTS", 500),
            heartbeat_file: var("WORKER_HEARTBEAT_FILE")
                .unwrap_or_else(|| "/tmp/rvmail-worker.heartbeat".into()),
            db_pool_size: num("WORKER_DB_POOL", 5),
        }
    }
}
