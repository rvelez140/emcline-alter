//! Cuentas de correo del usuario (solo con la sesión del navegador).

use super::auth::Session;
use super::clients::{self, humanize_smtp, Conn, Secret};
use super::error::{ApiError, ApiResult};
use super::state::Shared;
use super::validate::{self, Body};
use crate::net::imap::ImapError;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use futures::TryStreamExt;
use redis::AsyncCommands;
use rvmail_core::events::{WorkerControl, WORKER_CONTROL_CHANNEL};
use rvmail_core::models::MailAccount;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/accounts", get(list).post(create))
        .route("/accounts/test", post(test))
        .route("/accounts/{id}", patch(update).delete(remove))
}

/// Nunca devolver la credencial cifrada al cliente (regla 2).
pub fn dto(a: &MailAccount) -> Value {
    json!({
        "id": a.id, "userId": a.user_id, "displayName": a.display_name, "email": a.email, "color": a.color,
        "imapHost": a.imap_host, "imapPort": a.imap_port, "imapSecure": a.imap_secure,
        "smtpHost": a.smtp_host, "smtpPort": a.smtp_port, "smtpSecure": a.smtp_secure,
        "username": a.username, "authType": a.auth_type, "source": a.source, "signatureHtml": a.signature_html,
        "enabled": a.enabled, "status": a.status, "lastError": a.last_error, "lastSyncAt": a.last_sync_at,
        "sortOrder": a.sort_order, "createdAt": a.created_at,
    })
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConnectionInput {
    imap_host: String,
    #[serde(default = "d993")]
    imap_port: u16,
    #[serde(default = "dtrue")]
    imap_secure: bool,
    smtp_host: String,
    #[serde(default = "d465")]
    smtp_port: u16,
    #[serde(default = "dtrue")]
    smtp_secure: bool,
    username: String,
    password: String,
}
fn d993() -> u16 {
    993
}
fn d465() -> u16 {
    465
}
fn dtrue() -> bool {
    true
}

impl ConnectionInput {
    fn conn(&self) -> ApiResult<Conn> {
        for (f, v) in [
            ("imapHost", &self.imap_host),
            ("smtpHost", &self.smtp_host),
            ("username", &self.username),
        ] {
            validate::len(f, v.trim(), 1, 255)?;
        }
        validate::len("password", &self.password, 1, 1000)?;
        if self.imap_port == 0 || self.smtp_port == 0 {
            return Err(ApiError::bad_request("Datos inválidos — puerto no válido"));
        }
        Ok(Conn {
            imap_host: self.imap_host.trim().into(),
            imap_port: self.imap_port,
            imap_secure: self.imap_secure,
            smtp_host: self.smtp_host.trim().into(),
            smtp_port: self.smtp_port,
            smtp_secure: self.smtp_secure,
            username: self.username.trim().into(),
            secret: Secret::Password(self.password.clone()),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInput {
    #[serde(flatten)]
    conn: ConnectionInput,
    display_name: String,
    email: String,
    color: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UpdateInput {
    display_name: Option<String>,
    email: Option<String>,
    color: Option<String>,
    imap_host: Option<String>,
    imap_port: Option<u16>,
    imap_secure: Option<bool>,
    smtp_host: Option<String>,
    smtp_port: Option<u16>,
    smtp_secure: Option<bool>,
    username: Option<String>,
    password: Option<String>,
    enabled: Option<bool>,
    signature_html: Option<Option<String>>,
    sort_order: Option<i32>,
}

fn color(c: &Option<String>) -> ApiResult<()> {
    if let Some(c) = c {
        let ok =
            c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
        if !ok {
            return Err(ApiError::bad_request(
                "Datos inválidos — color: formato #rrggbb",
            ));
        }
    }
    Ok(())
}

/// Prueba IMAP y SMTP a la vez, sin guardar nada.
pub async fn test_connection(st: &Shared, c: &Conn) -> Value {
    let imap = async {
        let mut s = clients::imap(st, c).await.map_err(|e| match e {
            ImapError::Auth => "Usuario o contraseña rechazados por el servidor".to_string(),
            ImapError::Other(m) => m,
        })?;
        let r: Result<Vec<_>, _> =
            async { s.list(Some(""), Some("*")).await?.try_collect().await }.await;
        clients::close(s).await;
        r.map(|_| ())
            .map_err(|_| "El servidor IMAP no respondió correctamente".to_string())
    };
    let smtp = async {
        let t = clients::smtp(st, c).map_err(|e| e.message)?;
        match t.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => Err("El servidor SMTP no respondió".to_string()),
            Err(e) => Err(humanize_smtp(&e)),
        }
    };
    let (i, s) = tokio::join!(imap, smtp);
    let part = |r: Result<(), String>| match r {
        Ok(()) => json!({ "ok": true, "error": null }),
        Err(e) => json!({ "ok": false, "error": e }),
    };
    json!({ "imap": part(i), "smtp": part(s) })
}

fn failed(r: &Value) -> Option<String> {
    let ok = |k: &str| r[k]["ok"].as_bool().unwrap_or(false);
    if ok("imap") && ok("smtp") {
        return None;
    }
    let msg = |k: &str| r[k]["error"].as_str().unwrap_or("OK").to_string();
    Some(format!(
        "No se pudo conectar — IMAP: {} · SMTP: {}",
        msg("imap"),
        msg("smtp")
    ))
}

pub async fn notify(st: &Shared, msg: WorkerControl) {
    let mut r = st.redis.clone();
    let _: Result<i64, _> = r
        .publish(
            WORKER_CONTROL_CHANNEL,
            serde_json::to_string(&msg).expect("serializable"),
        )
        .await;
}

async fn list(State(st): State<Shared>, Session(u): Session) -> ApiResult<Json<Vec<Value>>> {
    let rows: Vec<MailAccount> = sqlx::query_as(
        "select * from mail_accounts where user_id = $1 order by sort_order, created_at",
    )
    .bind(u.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.iter().map(dto).collect()))
}

async fn test(
    State(st): State<Shared>,
    Session(_): Session,
    Body(i): Body<ConnectionInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(test_connection(&st, &i.conn()?).await))
}

async fn create(
    State(st): State<Shared>,
    Session(u): Session,
    Body(i): Body<CreateInput>,
) -> ApiResult<Json<Value>> {
    let name = i.display_name.trim().to_string();
    validate::len("displayName", &name, 1, 100)?;
    let email = validate::email("email", &i.email)?;
    color(&i.color)?;
    let conn = i.conn.conn()?;
    if let Some(m) = failed(&test_connection(&st, &conn).await) {
        return Err(ApiError::bad_request(m));
    }
    let ms = &st.cfg.mailserver;
    let source = if !ms.hostname.is_empty() && conn.imap_host == ms.hostname {
        "mailserver"
    } else {
        "external"
    };
    let row: MailAccount = sqlx::query_as(
        "insert into mail_accounts (user_id, display_name, email, color, imap_host, imap_port, imap_secure,
           smtp_host, smtp_port, smtp_secure, username, secret_enc, source)
         values ($1,$2,$3,coalesce($4,'#2563eb'),$5,$6,$7,$8,$9,$10,$11,$12,$13) returning *",
    )
    .bind(u.id)
    .bind(&name)
    .bind(&email)
    .bind(&i.color)
    .bind(&conn.imap_host)
    .bind(conn.imap_port as i32)
    .bind(conn.imap_secure)
    .bind(&conn.smtp_host)
    .bind(conn.smtp_port as i32)
    .bind(conn.smtp_secure)
    .bind(&conn.username)
    .bind(st.core.master_key.encrypt(&i.conn.password))
    .bind(source)
    .fetch_one(&st.db)
    .await?;
    notify(&st, WorkerControl::AccountChanged { account_id: row.id }).await;
    Ok(Json(dto(&row)))
}

async fn get_owned(st: &Shared, user: Uuid, id: Uuid) -> ApiResult<MailAccount> {
    sqlx::query_as("select * from mail_accounts where id = $1 and user_id = $2")
        .bind(id)
        .bind(user)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Cuenta no encontrada"))
}

async fn update(
    State(st): State<Shared>,
    Session(u): Session,
    Path(id): Path<Uuid>,
    Body(i): Body<UpdateInput>,
) -> ApiResult<Json<Value>> {
    let cur = get_owned(&st, u.id, id).await?;
    color(&i.color)?;
    if let Some(n) = &i.display_name {
        validate::len("displayName", n.trim(), 1, 100)?;
    }
    let email = i
        .email
        .as_deref()
        .map(|e| validate::email("email", e))
        .transpose()?;
    if let Some(Some(sig)) = &i.signature_html {
        validate::len("signatureHtml", sig, 0, 20_000)?;
    }
    let connection_changed = i.password.is_some()
        || i.imap_host.is_some()
        || i.imap_port.is_some()
        || i.imap_secure.is_some()
        || i.smtp_host.is_some()
        || i.smtp_port.is_some()
        || i.smtp_secure.is_some()
        || i.username.is_some();
    let mut secret_enc = cur.secret_enc.clone();
    let mut status = cur.status.clone();
    let mut last_error = cur.last_error.clone();
    if connection_changed && cur.auth_type == "password" {
        let password = match &i.password {
            Some(p) => p.clone(),
            None => st.core.master_key.decrypt(&cur.secret_enc)?,
        };
        let merged = ConnectionInput {
            imap_host: i.imap_host.clone().unwrap_or(cur.imap_host.clone()),
            imap_port: i.imap_port.unwrap_or(cur.imap_port as u16),
            imap_secure: i.imap_secure.unwrap_or(cur.imap_secure),
            smtp_host: i.smtp_host.clone().unwrap_or(cur.smtp_host.clone()),
            smtp_port: i.smtp_port.unwrap_or(cur.smtp_port as u16),
            smtp_secure: i.smtp_secure.unwrap_or(cur.smtp_secure),
            username: i.username.clone().unwrap_or(cur.username.clone()),
            password: password.clone(),
        };
        if let Some(m) = failed(&test_connection(&st, &merged.conn()?).await) {
            return Err(ApiError::bad_request(m));
        }
        if i.password.is_some() {
            secret_enc = st.core.master_key.encrypt(&password);
        }
        status = "pending".into();
        last_error = None;
    }
    let signature = match i.signature_html {
        Some(v) => v,
        None => cur.signature_html.clone(),
    };
    let row: MailAccount = sqlx::query_as(
        "update mail_accounts set display_name=$2, email=$3, color=$4, imap_host=$5, imap_port=$6, imap_secure=$7,
           smtp_host=$8, smtp_port=$9, smtp_secure=$10, username=$11, secret_enc=$12, enabled=$13, signature_html=$14,
           sort_order=$15, status=$16, last_error=$17
         where id = $1 returning *",
    )
    .bind(id)
    .bind(i.display_name.map(|n| n.trim().to_string()).unwrap_or(cur.display_name))
    .bind(email.unwrap_or(cur.email))
    .bind(i.color.unwrap_or(cur.color))
    .bind(i.imap_host.map(|h| h.trim().to_string()).unwrap_or(cur.imap_host))
    .bind(i.imap_port.map(i32::from).unwrap_or(cur.imap_port))
    .bind(i.imap_secure.unwrap_or(cur.imap_secure))
    .bind(i.smtp_host.map(|h| h.trim().to_string()).unwrap_or(cur.smtp_host))
    .bind(i.smtp_port.map(i32::from).unwrap_or(cur.smtp_port))
    .bind(i.smtp_secure.unwrap_or(cur.smtp_secure))
    .bind(i.username.map(|h| h.trim().to_string()).unwrap_or(cur.username))
    .bind(secret_enc)
    .bind(i.enabled.unwrap_or(cur.enabled))
    .bind(signature)
    .bind(i.sort_order.unwrap_or(cur.sort_order))
    .bind(status)
    .bind(last_error)
    .fetch_one(&st.db)
    .await?;
    notify(&st, WorkerControl::AccountChanged { account_id: id }).await;
    Ok(Json(dto(&row)))
}

async fn remove(
    State(st): State<Shared>,
    Session(u): Session,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    get_owned(&st, u.id, id).await?;
    sqlx::query("delete from mail_accounts where id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    notify(&st, WorkerControl::AccountDeleted { account_id: id }).await;
    Ok(StatusCode::NO_CONTENT)
}
