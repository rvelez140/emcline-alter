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

/// Servidor de correo integrado (aaPanel hoy; Stalwart en la fase J).
#[derive(Clone, Debug)]
pub struct MailServerConfig {
    /// none = solo cliente; aapanel = administra el Mail Server de aaPanel del mismo VPS
    pub provider: String,
    pub hostname: String,
    pub imap_port: u16,
    pub smtp_port: u16,
    pub aapanel_url: String,
    pub aapanel_api_key: String,
    pub aapanel_verify_tls: bool,
}

/// Ajustes de la API HTTP.
#[derive(Clone)]
pub struct ApiConfig {
    pub port: u16,
    pub jwt_secret: String,
    pub allow_signup: bool,
    pub run_migrations: bool,
    pub secure_cookies: bool,
    pub session_days: i64,
    pub app_url: String,
    pub agent_send_limit_per_hour: i64,
    pub append_sent: bool,
    /// Carpeta con la web compilada (apps/web/dist). Vacío = no servir estáticos.
    pub web_dir: String,
    pub version: String,
    pub db_pool_size: u32,
    pub mailserver: MailServerConfig,
}

impl ApiConfig {
    pub fn from_env() -> Result<Self> {
        let production = var("NODE_ENV").as_deref() == Some("production")
            || var("RVMAIL_ENV").as_deref() == Some("production");
        Ok(Self {
            port: num("API_PORT", 3000),
            jwt_secret: required("JWT_SECRET")?,
            allow_signup: flag("ALLOW_SIGNUP"),
            run_migrations: var("RUN_MIGRATIONS").as_deref() != Some("false"),
            secure_cookies: production && !flag("INSECURE_COOKIES"),
            session_days: num("SESSION_DAYS", 30),
            app_url: var("APP_URL").unwrap_or_else(|| "http://localhost:5173".into()),
            agent_send_limit_per_hour: num("AGENT_SEND_LIMIT_PER_HOUR", 30),
            append_sent: var("APPEND_SENT").as_deref() != Some("false"),
            web_dir: var("WEB_DIR").unwrap_or_default(),
            version: var("APP_VERSION").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
            db_pool_size: num("API_DB_POOL", 10),
            mailserver: MailServerConfig {
                provider: var("MAILSERVER_PROVIDER").unwrap_or_else(|| "none".into()),
                hostname: var("MAILSERVER_HOSTNAME").unwrap_or_default(),
                imap_port: num("MAILSERVER_IMAP_PORT", 993),
                smtp_port: num("MAILSERVER_SMTP_PORT", 465),
                aapanel_url: var("AAPANEL_URL")
                    .unwrap_or_else(|| "https://host.docker.internal:7800".into())
                    .trim_end_matches('/')
                    .to_string(),
                aapanel_api_key: var("AAPANEL_API_KEY").unwrap_or_default(),
                aapanel_verify_tls: flag("AAPANEL_VERIFY_TLS"),
            },
        })
    }
}
