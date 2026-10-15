//! Filas de la base de datos (esquema en `packages/db/src/schema.ts` y migraciones SQL).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Address {
    pub name: Option<String>,
    pub address: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct MailAccount {
    pub id: Uuid,
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    pub color: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub imap_secure: bool,
    pub smtp_host: String,
    pub smtp_port: i32,
    pub smtp_secure: bool,
    pub username: String,
    pub secret_enc: String,
    pub auth_type: String,
    pub source: String,
    pub oauth_provider: Option<String>,
    pub signature_html: Option<String>,
    pub enabled: bool,
    pub status: String,
    pub last_error: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Folder {
    pub id: Uuid,
    pub account_id: Uuid,
    pub path: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub special_use: Option<String>,
    pub subscribed: bool,
    pub uid_validity: Option<i64>,
    pub last_uid: i64,
    pub highest_modseq: Option<String>,
    pub unread_count: i32,
    pub total_count: i32,
    pub synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    /// None en usuarios creados por SSO sin contraseña local
    pub password_hash: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// Ámbitos que puede tener un token de API (agentes de IA, integraciones).
pub const API_SCOPES: [&str; 4] = ["mail:read", "mail:draft", "mail:send", "mail:modify"];

#[derive(Debug, Clone, FromRow)]
pub struct ApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub prefix: String,
    pub scopes: sqlx::types::Json<Vec<String>>,
    pub account_ids: Option<sqlx::types::Json<Vec<Uuid>>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
