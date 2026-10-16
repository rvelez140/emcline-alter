//! Operaciones de correo para cuentas JMAP (las de IMAP están en mail.rs).

use super::error::{ApiError, ApiResult};
use super::mail::{AttachmentInfo, ComposeInput, MessageBody};
use super::state::Shared;
use crate::jmap::{Auth, Client, JmapError};
use anyhow::Context;
use rvmail_core::models::MailAccount;
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub async fn client(st: &Shared, a: &MailAccount) -> ApiResult<Client> {
    let auth = if a.auth_type == "oauth2" {
        Auth::Bearer(super::sso::access_token_for(st, a).await?)
    } else {
        Auth::Basic {
            user: a.username.clone(),
            pass: st.core.master_key.decrypt(&a.secret_enc)?,
        }
    };
    Client::connect(&st.http, a.jmap_url.as_deref().unwrap_or_default(), auth)
        .await
        .map_err(|e| match e {
            JmapError::Auth => ApiError::bad_gateway("El servidor rechazó usuario o contraseña"),
            JmapError::Other(m) => ApiError::bad_gateway(m),
        })
}

fn fail(e: anyhow::Error) -> ApiError {
    tracing::warn!(err = %format!("{e:#}"), "operación JMAP falló");
    ApiError::bad_gateway("El servidor de correo (JMAP) no pudo completar la operación")
}

/// Cuerpo de un correo: HTML, texto y adjuntos.
pub async fn body(st: &Shared, a: &MailAccount, remote_id: &str) -> ApiResult<MessageBody> {
    let c = client(st, a).await?;
    let e = c
        .get_body(remote_id)
        .await
        .map_err(fail)?
        .ok_or_else(|| ApiError::not_found("El mensaje ya no existe en el servidor"))?;
    let values = e.get("bodyValues").cloned().unwrap_or_default();
    let join = |key: &str, mime: &str| -> Option<String> {
        let parts = e.get(key)?.as_array()?;
        let text: Vec<String> = parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some(mime))
            .filter_map(|p| {
                values
                    .get(p.get("partId")?.as_str()?)?
                    .get("value")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        (!text.is_empty()).then(|| text.join("\n"))
    };
    let attachments = e
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|p| AttachmentInfo {
                    filename: p.get("name").and_then(Value::as_str).map(str::to_string),
                    content_type: p
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    size: p.get("size").and_then(Value::as_u64).unwrap_or(0) as usize,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(MessageBody {
        html: join("htmlBody", "text/html"),
        text: join("textBody", "text/plain"),
        attachments,
    })
}

pub async fn set_flags(
    st: &Shared,
    a: &MailAccount,
    remote_id: &str,
    seen: Option<bool>,
    flagged: Option<bool>,
) -> ApiResult<()> {
    let mut patch = Map::new();
    if let Some(s) = seen {
        patch.insert(
            "keywords/$seen".into(),
            if s { json!(true) } else { Value::Null },
        );
    }
    if let Some(f) = flagged {
        patch.insert(
            "keywords/$flagged".into(),
            if f { json!(true) } else { Value::Null },
        );
    }
    client(st, a)
        .await?
        .set_email(json!({ "update": { remote_id: patch } }))
        .await
        .map_err(fail)?;
    Ok(())
}

/// A la papelera (su id JMAP es `folders.path`) o borrado definitivo.
pub async fn remove(
    st: &Shared,
    a: &MailAccount,
    remote_id: &str,
    trash_mailbox: Option<&str>,
) -> ApiResult<()> {
    let c = client(st, a).await?;
    let req = match trash_mailbox {
        Some(t) => json!({ "update": { remote_id: { "mailboxIds": { t: true } } } }),
        None => json!({ "destroy": [remote_id] }),
    };
    c.set_email(req).await.map_err(fail)?;
    Ok(())
}

fn email_object(
    a: &MailAccount,
    i: &ComposeInput,
    html: &str,
    text: &str,
    reply: Option<&str>,
) -> Value {
    let addrs = |l: &[String]| l.iter().map(|e| json!({ "email": e })).collect::<Vec<_>>();
    let mut e = json!({
        "from": [{ "name": a.display_name, "email": a.email }],
        "to": addrs(&i.to),
        "cc": addrs(&i.cc),
        "bcc": addrs(&i.bcc),
        "subject": i.subject,
        "bodyValues": { "t": { "value": text }, "h": { "value": html } },
        "textBody": [{ "partId": "t", "type": "text/plain" }],
        "htmlBody": [{ "partId": "h", "type": "text/html" }],
    });
    if let Some(r) = reply {
        let id = r.trim_start_matches('<').trim_end_matches('>');
        e["inReplyTo"] = json!([id]);
        e["references"] = json!([id]);
    }
    e
}

pub async fn send(
    st: &Shared,
    a: &MailAccount,
    i: &ComposeInput,
    html: &str,
    text: &str,
    reply: Option<&str>,
) -> ApiResult<String> {
    let sent = mailbox_by_role(st, a.id, "\\Sent")
        .await?
        .ok_or_else(|| ApiError::bad_gateway("La cuenta no tiene carpeta de Enviados"))?;
    let c = client(st, a).await?;
    let id = c
        .submit(email_object(a, i, html, text, reply), &sent, &a.email)
        .await
        .map_err(|e| {
            tracing::warn!(err = %format!("{e:#}"), "envío JMAP falló");
            ApiError::bad_gateway(format!("No se pudo enviar: {}", short(&e)))
        })?;
    Ok(id)
}

pub async fn save_draft(
    st: &Shared,
    a: &MailAccount,
    i: &ComposeInput,
    html: &str,
    text: &str,
    reply: Option<&str>,
) -> ApiResult<String> {
    let drafts = mailbox_by_role(st, a.id, "\\Drafts")
        .await?
        .ok_or_else(|| ApiError::bad_gateway("La cuenta no tiene carpeta de Borradores"))?;
    let mut e = email_object(a, i, html, text, reply);
    e["mailboxIds"] = json!({ drafts.clone(): true });
    e["keywords"] = json!({ "$draft": true, "$seen": true });
    let c = client(st, a).await?;
    c.set_email(json!({ "create": { "d1": e } }))
        .await
        .map_err(fail)?;
    Ok(drafts)
}

async fn mailbox_by_role(st: &Shared, account: Uuid, special: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("select path from folders where account_id = $1 and special_use = $2")
            .bind(account)
            .bind(special)
            .fetch_optional(&st.db)
            .await?,
    )
}

fn short(e: &anyhow::Error) -> String {
    let s = e.to_string();
    if s.chars().count() > 160 {
        "el servidor rechazó el mensaje".into()
    } else {
        s
    }
}

/// Prueba una conexión JMAP antes de guardar la cuenta.
pub async fn test(st: &Shared, url: &str, user: &str, pass: &str) -> Result<(), String> {
    let c = Client::connect(
        &st.http,
        url,
        Auth::Basic {
            user: user.into(),
            pass: pass.into(),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    c.mailboxes()
        .await
        .context("Mailbox/get")
        .map_err(|_| "El servidor JMAP no devolvió las carpetas".to_string())?;
    Ok(())
}
