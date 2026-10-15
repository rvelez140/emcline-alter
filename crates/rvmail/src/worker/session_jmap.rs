//! Sincronización de cuentas JMAP (RFC 8621): misma tabla de carpetas y
//! mensajes que IMAP; los avisos llegan por EventSource (push) en lugar de IDLE.

use super::super::parse::{normalize_subject, MessageRow};
use super::{insert_rows, publish, AccountSession, Command, Outcome, UserFacing};
use crate::jmap::{self, Auth, Client, JmapError};
use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rvmail_core::events::{AccountStatus, RvEvent};
use rvmail_core::models::{Address, MailAccount};
use serde_json::Value;
use sqlx::types::Json;
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};
use uuid::Uuid;

const PROPS: [&str; 14] = [
    "id",
    "mailboxIds",
    "keywords",
    "receivedAt",
    "sentAt",
    "subject",
    "from",
    "to",
    "cc",
    "preview",
    "hasAttachment",
    "size",
    "messageId",
    "inReplyTo",
];

/// Palabras clave JMAP → marcas IMAP (la BD y la web usan las de IMAP).
pub fn keywords_to_flags(k: Option<&Value>) -> Vec<String> {
    let Some(map) = k.and_then(Value::as_object) else {
        return vec![];
    };
    map.iter()
        .filter(|(_, v)| v.as_bool() == Some(true))
        .map(|(k, _)| match k.as_str() {
            "$seen" => "\\Seen".to_string(),
            "$flagged" => "\\Flagged".into(),
            "$answered" => "\\Answered".into(),
            "$draft" => "\\Draft".into(),
            other => other.to_string(),
        })
        .collect()
}

fn addresses(v: Option<&Value>) -> Vec<Address> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Address {
                        name: x
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|n| !n.is_empty())
                            .map(str::to_string),
                        address: x.get("email")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn first_id(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(|id| format!("<{id}>"))
}

pub fn email_to_row(account_id: Uuid, folder_id: Uuid, e: &Value) -> Option<MessageRow> {
    let id = e.get("id")?.as_str()?.to_string();
    let subject = e
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let from = addresses(e.get("from")).into_iter().next();
    let date = ["sentAt", "receivedAt"]
        .iter()
        .find_map(|k| {
            e.get(*k)
                .and_then(Value::as_str)
                .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
        })
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let flags = keywords_to_flags(e.get("keywords"));
    Some(MessageRow {
        account_id,
        folder_id,
        uid: jmap::stable_uid(&id),
        remote_id: Some(id),
        message_id: first_id(e.get("messageId")),
        in_reply_to: first_id(e.get("inReplyTo")),
        thread_key: normalize_subject(&subject),
        from_name: from.as_ref().and_then(|a| a.name.clone()),
        from_address: from.map(|a| a.address),
        to: addresses(e.get("to")),
        cc: addresses(e.get("cc")),
        subject,
        date,
        snippet: e
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(240)
            .collect(),
        seen: flags.iter().any(|f| f == "\\Seen"),
        flagged: flags.iter().any(|f| f == "\\Flagged"),
        flags,
        has_attachments: e
            .get("hasAttachment")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        size: e.get("size").and_then(Value::as_u64).unwrap_or(0) as u32,
    })
}

/// Credencial JMAP de la cuenta: contraseña (Basic) o token OAuth (Bearer).
pub async fn auth_for(ctx: &super::Ctx, a: &MailAccount) -> Result<Auth, Outcome2> {
    if a.auth_type == "oauth2" {
        match crate::oauth::access_token(&ctx.db, &ctx.redis, &ctx.core.master_key, &ctx.http, a)
            .await
        {
            Ok(t) => Ok(Auth::Bearer(t)),
            Err(crate::oauth::OAuthError::Revoked) => Err(Outcome2::AuthFailed),
            Err(crate::oauth::OAuthError::Other(m)) => Err(Outcome2::Error(m)),
        }
    } else {
        let pass = ctx
            .core
            .master_key
            .decrypt(&a.secret_enc)
            .map_err(|e| Outcome2::Error(e.to_string()))?;
        Ok(Auth::Basic {
            user: a.username.clone(),
            pass,
        })
    }
}

pub enum Outcome2 {
    AuthFailed,
    Error(String),
}

impl AccountSession {
    pub(super) async fn serve_jmap(&mut self, account: MailAccount) -> Result<Outcome> {
        let url = account.jmap_url.clone().unwrap_or_default();
        let auth = match auth_for(&self.ctx, &account).await {
            Ok(a) => a,
            Err(Outcome2::AuthFailed) => return Ok(Outcome::AuthFailed),
            Err(Outcome2::Error(m)) => return Err(UserFacing(m).into()),
        };
        let client = match Client::connect(&self.ctx.http, &url, auth).await {
            Ok(c) => c,
            Err(JmapError::Auth) => return Ok(Outcome::AuthFailed),
            Err(JmapError::Other(m)) => return Err(UserFacing(m).into()),
        };
        let mut sync = JmapSync {
            account: &account,
            ctx: &self.ctx,
            client: &client,
            last_email_state: None,
        };
        sync.full().await?;
        self.set_status(AccountStatus::Idle, None).await;
        info!(account = %account.email, "cuenta JMAP sincronizada");

        loop {
            // Push: el servidor avisa de cambios; si no hay avisos, repaso periódico
            let wait = client.wait_change(self.ctx.cfg.sync_interval);
            tokio::select! {
                r = wait => match r {
                    Ok(_) => sync.full().await?,
                    Err(e) => {
                        // EventSource no disponible: sondeo periódico
                        warn!(account = %account.email, err = %e, "EventSource JMAP no disponible, se usa sondeo");
                        tokio::select! {
                            _ = tokio::time::sleep(self.ctx.cfg.sync_interval) => sync.full().await?,
                            cmd = self.rx.recv() => match cmd {
                                Some(Command::Sync) => sync.full().await?,
                                None | Some(Command::Stop) => return Ok(Outcome::Stopped),
                            },
                        }
                    }
                },
                cmd = self.rx.recv() => match cmd {
                    Some(Command::Sync) => sync.full().await?,
                    None | Some(Command::Stop) => return Ok(Outcome::Stopped),
                },
            }
        }
    }
}

struct JmapSync<'a> {
    account: &'a MailAccount,
    ctx: &'a super::Ctx,
    client: &'a Client,
    /// Estado de Email en la última pasada: si no cambió, no hace falta revisar marcas
    last_email_state: Option<String>,
}

impl JmapSync<'_> {
    async fn full(&mut self) -> Result<()> {
        let db = &self.ctx.db;
        let (mailboxes, _) = self.client.mailboxes().await?;
        let paths = jmap::mailbox_paths(&mailboxes);
        let mut folders: Vec<(Uuid, String, Option<String>)> = vec![];
        for m in &mailboxes {
            let Some(id) = m.get("id").and_then(Value::as_str) else {
                continue;
            };
            let special = jmap::role_to_special(m.get("role").and_then(Value::as_str));
            let name = paths.get(id).cloned().unwrap_or_else(|| id.to_string());
            let folder_id: Uuid = sqlx::query_scalar(
                "insert into folders (account_id, path, name, delimiter, special_use, total_count, unread_count)
                 values ($1, $2, $3, '/', $4, $5, $6)
                 on conflict (account_id, path) do update set name = excluded.name, special_use = excluded.special_use,
                   total_count = excluded.total_count, unread_count = excluded.unread_count
                 returning id",
            )
            .bind(self.account.id)
            .bind(id)
            .bind(&name)
            .bind(&special)
            .bind(m.get("totalEmails").and_then(Value::as_i64).unwrap_or(0) as i32)
            .bind(m.get("unreadEmails").and_then(Value::as_i64).unwrap_or(0) as i32)
            .fetch_one(db)
            .await?;
            folders.push((folder_id, id.to_string(), special));
        }
        let ids: Vec<&str> = folders.iter().map(|f| f.1.as_str()).collect();
        sqlx::query("delete from folders where account_id = $1 and path <> all($2)")
            .bind(self.account.id)
            .bind(&ids)
            .execute(db)
            .await?;

        let since = Utc::now() - ChronoDuration::days(self.ctx.cfg.sync_days);
        let after = since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut email_state = None;
        for (folder_id, mailbox, _) in &folders {
            let remote: Vec<String> = self.client.query_ids(mailbox, &after).await?;
            let remote_set: HashSet<&str> = remote.iter().map(String::as_str).collect();
            let local: Vec<(Uuid, Option<String>, Json<Vec<String>>)> = sqlx::query_as(
                "select id, remote_id, flags from messages where folder_id = $1 and date >= $2",
            )
            .bind(folder_id)
            .bind(since - ChronoDuration::days(1))
            .fetch_all(db)
            .await?;
            let local_by_remote: HashMap<&str, (Uuid, &Vec<String>)> = local
                .iter()
                .filter_map(|(id, r, f)| Some((r.as_deref()?, (*id, &f.0))))
                .collect();

            let gone: Vec<Uuid> = local
                .iter()
                .filter(|(_, r, _)| r.as_deref().is_none_or(|r| !remote_set.contains(r)))
                .map(|(id, _, _)| *id)
                .collect();
            if !gone.is_empty() {
                sqlx::query("delete from messages where id = any($1)")
                    .bind(&gone)
                    .execute(db)
                    .await?;
            }
            let new: Vec<String> = remote
                .iter()
                .filter(|r| !local_by_remote.contains_key(r.as_str()))
                .cloned()
                .collect();
            let mut inserted = 0;
            if !new.is_empty() {
                let (emails, state) = self.client.get_emails(&new, &PROPS).await?;
                email_state = Some(state);
                let rows: Vec<MessageRow> = emails
                    .iter()
                    .filter_map(|e| email_to_row(self.account.id, *folder_id, e))
                    .collect();
                inserted = insert_rows(db, &rows).await?;
            }

            // Marcas de los que ya teníamos (solo si algo cambió en el servidor)
            let mut updated = vec![];
            let known: Vec<String> = remote
                .iter()
                .filter(|r| local_by_remote.contains_key(r.as_str()))
                .cloned()
                .collect();
            let probe_state = self
                .client
                .get_emails(&[], &["id"])
                .await
                .map(|(_, s)| s)
                .ok();
            if !known.is_empty()
                && (self.last_email_state.is_none() || probe_state != self.last_email_state)
            {
                let (emails, _) = self.client.get_emails(&known, &["id", "keywords"]).await?;
                for e in &emails {
                    let Some(rid) = e.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some((id, old)) = local_by_remote.get(rid) else {
                        continue;
                    };
                    let flags = keywords_to_flags(e.get("keywords"));
                    let mut a = flags.clone();
                    let mut b = (*old).clone();
                    a.sort();
                    b.sort();
                    if a != b {
                        sqlx::query(
                            "update messages set flags = $2, seen = $3, flagged = $4 where id = $1",
                        )
                        .bind(id)
                        .bind(Json(&flags))
                        .bind(flags.iter().any(|x| x == "\\Seen"))
                        .bind(flags.iter().any(|x| x == "\\Flagged"))
                        .execute(db)
                        .await?;
                        updated.push(*id);
                    }
                }
            }
            if email_state.is_none() {
                email_state = probe_state;
            }

            let user = self.account.user_id;
            if inserted > 0 {
                publish(
                    &self.ctx.redis,
                    user,
                    &RvEvent::MessageNew {
                        account_id: self.account.id,
                        folder_id: *folder_id,
                        count: inserted,
                    },
                )
                .await;
            }
            if !updated.is_empty() {
                publish(
                    &self.ctx.redis,
                    user,
                    &RvEvent::MessageUpdated {
                        account_id: self.account.id,
                        message_ids: updated,
                    },
                )
                .await;
            }
            if inserted > 0 || !gone.is_empty() {
                publish(
                    &self.ctx.redis,
                    user,
                    &RvEvent::FolderUpdated {
                        account_id: self.account.id,
                        folder_id: *folder_id,
                    },
                )
                .await;
            }
        }
        self.last_email_state = email_state;
        sqlx::query("update mail_accounts set last_sync_at = now() where id = $1")
            .bind(self.account.id)
            .execute(db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_jmap_email() {
        let e = json!({
            "id": "Mabc", "keywords": {"$seen": true, "$flagged": true}, "subject": "Re: Cotización",
            "from": [{"name": "José", "email": "jose@x.com"}], "to": [{"name": null, "email": "a@x.com"}],
            "sentAt": "2026-10-15T10:00:00Z", "preview": "hola", "hasAttachment": true, "size": 1200,
            "messageId": ["id1@x"], "inReplyTo": null
        });
        let r = email_to_row(Uuid::nil(), Uuid::nil(), &e).unwrap();
        assert_eq!(r.remote_id.as_deref(), Some("Mabc"));
        assert_eq!(r.thread_key, "cotización");
        assert_eq!(r.from_name.as_deref(), Some("José"));
        assert!(r.seen && r.flagged && r.has_attachments);
        assert_eq!(r.message_id.as_deref(), Some("<id1@x>"));
        assert!(r.in_reply_to.is_none());
    }
}
