//! Tipos de la API HTTP de RV Mail (lo que viaja en JSON).
//!
//! Los usan el servidor (`crates/rvmail`, con la característica `sqlx`) y los
//! clientes en Rust (web Leptos en WebAssembly, apps): si un campo cambia aquí,
//! el compilador avisa en ambos lados.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Columna JSON de Postgres en el servidor; un valor normal en los clientes.
/// Se serializa igual en los dos casos.
#[cfg(feature = "sqlx")]
pub type JsonCol<T> = sqlx::types::Json<T>;
#[cfg(not(feature = "sqlx"))]
pub type JsonCol<T> = T;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Address {
    pub name: Option<String>,
    pub address: String,
}

/// Usuario de RV Mail (`/api/auth/me`, login).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserDto {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    /// "admin" | "user"
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginInput {
    pub email: String,
    pub password: String,
}

/// Error de la API (mensaje en español para mostrar al usuario).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorBody {
    pub status_code: u16,
    pub error: String,
    pub message: String,
}

/// Cuenta de correo (`/api/accounts`). Nunca incluye la credencial (regla 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
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
    /// password | oauth2
    pub auth_type: String,
    /// imap | jmap
    pub protocol: String,
    pub jmap_url: Option<String>,
    /// manual | mailserver
    pub source: String,
    pub signature_html: Option<String>,
    pub enabled: bool,
    /// idle | syncing | ok | error | auth_error
    pub status: String,
    pub last_error: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

/// Carpeta (`/api/folders`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub id: Uuid,
    pub account_id: Uuid,
    pub path: String,
    pub name: String,
    pub delimiter: Option<String>,
    /// \Inbox, \Sent, \Drafts, \Trash, \Junk, \Archive…
    pub special_use: Option<String>,
    pub unread_count: i32,
    pub total_count: i32,
}

/// Mensaje en una lista (`/api/messages`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub id: Uuid,
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub to: JsonCol<Vec<Address>>,
    pub date: DateTime<Utc>,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub account_color: String,
    pub account_email: String,
}

/// Página de mensajes con cursor (`nextCursor` = None al final).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub items: Vec<MessageSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub filename: Option<String>,
    pub content_type: String,
    pub size: usize,
}

/// Cuerpo de un mensaje (HTML sin sanear: el cliente lo sanea siempre).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBody {
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Mensaje completo (`/api/messages/{id}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFull {
    pub id: Uuid,
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub message_id: Option<String>,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: DateTime<Utc>,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub account_color: String,
    pub account_email: String,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Vistas unificadas (todas las cuentas) por carpeta especial.
pub const SPECIALS: [(&str, &str); 5] = [
    ("inbox", "Bandeja de entrada"),
    ("sent", "Enviados"),
    ("archive", "Archivo"),
    ("junk", "Spam"),
    ("trash", "Papelera"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_json_is_camel_case() {
        let json = r##"{"id":"00000000-0000-0000-0000-000000000001","accountId":"00000000-0000-0000-0000-000000000002",
          "folderId":"00000000-0000-0000-0000-000000000003","subject":"Hola","fromName":"Ana","fromAddress":"ana@x.com",
          "to":[{"name":null,"address":"b@x.com"}],"date":"2026-10-15T10:00:00Z","snippet":"…","seen":false,
          "flagged":true,"hasAttachments":false,"accountColor":"#2563eb","accountEmail":"b@x.com"}"##;
        let m: MessageSummary = serde_json::from_str(json).unwrap();
        assert!(m.flagged && !m.seen);
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(back["hasAttachments"], false);
        assert_eq!(back["to"][0]["address"], "b@x.com");
    }
}
