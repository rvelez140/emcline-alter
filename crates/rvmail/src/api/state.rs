//! Estado compartido de la API.

use super::mailserver::MailServerService;
use redis::aio::ConnectionManager;
use rvmail_core::config::{ApiConfig, CoreConfig};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

pub struct AppState {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub core: CoreConfig,
    pub cfg: ApiConfig,
    pub mailserver: MailServerService,
    /// Cliente HTTP con TLS estricto (OAuth/OIDC)
    pub http: reqwest::Client,
    /// Eventos de Redis (`rvmail:events:<user>`) repartidos en memoria a cada SSE
    pub events: broadcast::Sender<(Uuid, String)>,
}

pub type Shared = Arc<AppState>;
