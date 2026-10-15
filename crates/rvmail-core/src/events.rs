//! Eventos en tiempo real worker → api → navegador (Redis pub/sub + SSE) y
//! mensajes de control api → worker. Mismo JSON que `packages/shared/src/events.ts`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const WORKER_CONTROL_CHANNEL: &str = "rvmail:worker:control";

pub fn events_channel(user_id: Uuid) -> String {
    format!("rvmail:events:{user_id}")
}

/// Clave de lease que garantiza un solo worker por cuenta.
pub fn account_lease_key(account_id: Uuid) -> String {
    format!("rvmail:lease:account:{account_id}")
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Pending,
    Syncing,
    Idle,
    AuthError,
    Error,
    Disabled,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Syncing => "syncing",
            Self::Idle => "idle",
            Self::AuthError => "auth_error",
            Self::Error => "error",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RvEvent {
    #[serde(rename = "message.new", rename_all = "camelCase")]
    MessageNew {
        account_id: Uuid,
        folder_id: Uuid,
        count: usize,
    },
    #[serde(rename = "message.updated", rename_all = "camelCase")]
    MessageUpdated {
        account_id: Uuid,
        message_ids: Vec<Uuid>,
    },
    #[serde(rename = "folder.updated", rename_all = "camelCase")]
    FolderUpdated { account_id: Uuid, folder_id: Uuid },
    #[serde(rename = "account.status", rename_all = "camelCase")]
    AccountStatus {
        account_id: Uuid,
        status: AccountStatus,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum WorkerControl {
    #[serde(rename = "account.changed", rename_all = "camelCase")]
    AccountChanged { account_id: Uuid },
    /// Pasada de sincronización sin reiniciar la sesión (tras enviar, mover, borrar)
    #[serde(rename = "account.sync", rename_all = "camelCase")]
    AccountSync { account_id: Uuid },
    #[serde(rename = "account.deleted", rename_all = "camelCase")]
    AccountDeleted { account_id: Uuid },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_matches_node() {
        let id = Uuid::nil();
        let e = RvEvent::MessageNew {
            account_id: id,
            folder_id: id,
            count: 2,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"type":"message.new","accountId":"00000000-0000-0000-0000-000000000000","folderId":"00000000-0000-0000-0000-000000000000","count":2}"#
        );
        let c: WorkerControl = serde_json::from_str(
            r#"{"type":"account.sync","accountId":"00000000-0000-0000-0000-000000000000"}"#,
        )
        .unwrap();
        assert_eq!(c, WorkerControl::AccountSync { account_id: id });
        let s = RvEvent::AccountStatus {
            account_id: id,
            status: AccountStatus::AuthError,
            error: None,
        };
        assert!(serde_json::to_string(&s)
            .unwrap()
            .contains(r#""status":"auth_error""#));
    }
}
