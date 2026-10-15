//! Sesiones de apps nativas (escritorio y móvil, fase C1).
//!
//! - `POST /auth/token`: correo + contraseña → access token (JWT, 15 min) +
//!   refresh token (`rvr_…`, 60 días, rota en cada uso).
//! - `POST /auth/token/refresh`, `POST /auth/token/revoke`.
//! - `POST /auth/token/exchange`: código de un solo uso del inicio de sesión con
//!   Google/Microsoft/Yahoo (redirección a `rvmail://…`, con PKCE).
//! - `GET/DELETE /auth/sessions`: dispositivos conectados.
//!
//! Si alguien presenta un refresh token ya rotado (robado y usado antes), se
//! revoca toda la sesión de ese dispositivo.

use super::auth::{
    self, app_revoked_key, authenticate, hash_token, sign_app_access, Session, APP_ACCESS_SECS,
};
use super::error::{ApiError, ApiResult};
use super::state::Shared;
use super::validate::{self, Body};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use rvmail_core::models::User;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use uuid::Uuid;

pub const REFRESH_PREFIX: &str = "rvr_";
const REFRESH_DAYS: i64 = 60;
/// Vida del código de un solo uso del SSO para apps.
pub const APP_CODE_TTL: u64 = 120;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/auth/token", post(issue))
        .route("/auth/token/refresh", post(refresh))
        .route("/auth/token/revoke", post(revoke))
        .route("/auth/token/exchange", post(exchange))
        .route("/auth/sessions", get(list))
        .route("/auth/sessions/{id}", delete(remove))
}

fn new_refresh() -> String {
    format!("{REFRESH_PREFIX}{}", crate::oauth::random_token(32))
}

fn tokens_json(st: &Shared, user: &User, sid: Uuid, refresh: &str) -> Value {
    json!({
        "accessToken": sign_app_access(st, user, sid),
        "refreshToken": refresh,
        "tokenType": "Bearer",
        "expiresIn": APP_ACCESS_SECS,
        "user": auth::user_json(user),
    })
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(300).collect())
}

/// Crea la sesión del dispositivo y devuelve sus tokens.
async fn open_session(
    st: &Shared,
    user: &User,
    device: Option<String>,
    platform: Option<String>,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> ApiResult<Value> {
    let device = device
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "App".into());
    validate::len("deviceName", &device, 1, 100)?;
    let platform = platform.unwrap_or_else(|| "desconocida".into());
    validate::len("platform", &platform, 1, 30)?;
    let refresh = new_refresh();
    let sid: Uuid = sqlx::query_scalar(
        "insert into app_sessions (user_id, device_name, platform, refresh_hash, user_agent, ip, expires_at)
         values ($1, $2, $3, $4, $5, $6, now() + make_interval(days => $7)) returning id",
    )
    .bind(user.id)
    .bind(&device)
    .bind(&platform)
    .bind(hash_token(&refresh))
    .bind(user_agent(headers))
    .bind(auth::client_ip(headers, peer))
    .bind(REFRESH_DAYS as i32)
    .fetch_one(&st.db)
    .await?;
    Ok(tokens_json(st, user, sid, &refresh))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueInput {
    email: String,
    password: String,
    device_name: Option<String>,
    platform: Option<String>,
}

async fn issue(
    State(st): State<Shared>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Body(i): Body<IssueInput>,
) -> ApiResult<Json<Value>> {
    let user = authenticate(&st, &headers, Some(peer), &i.email, &i.password).await?;
    Ok(Json(
        open_session(&st, &user, i.device_name, i.platform, &headers, Some(peer)).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshInput {
    refresh_token: String,
}

async fn refresh(State(st): State<Shared>, Body(i): Body<RefreshInput>) -> ApiResult<Json<Value>> {
    let invalid = || ApiError::unauthorized("Sesión no válida o expirada; vuelve a iniciar sesión");
    if !i.refresh_token.starts_with(REFRESH_PREFIX) {
        return Err(invalid());
    }
    let hash = hash_token(&i.refresh_token);
    let mut tx = st.db.begin().await?;
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        user_id: Uuid,
        expires_at: DateTime<Utc>,
        revoked_at: Option<DateTime<Utc>>,
    }
    let row: Option<Row> = sqlx::query_as(
        "select id, user_id, expires_at, revoked_at from app_sessions where refresh_hash = $1 for update",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        // ¿Token ya rotado? Alguien lo copió: se revoca la sesión entera
        let reused: Option<Uuid> = sqlx::query_scalar(
            "update app_sessions set revoked_at = now() where prev_refresh_hash = $1 and revoked_at is null returning id",
        )
        .bind(&hash)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some(sid) = reused {
            tracing::warn!(session = %sid, "refresh token reutilizado: sesión revocada");
            mark_revoked(&st, sid).await?;
        }
        return Err(invalid());
    };
    if row.revoked_at.is_some() || row.expires_at < Utc::now() {
        return Err(invalid());
    }
    let user: Option<User> = sqlx::query_as("select * from users where id = $1")
        .bind(row.user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let user = user.ok_or_else(invalid)?;
    let next = new_refresh();
    sqlx::query(
        "update app_sessions set prev_refresh_hash = refresh_hash, refresh_hash = $2, last_used_at = now(),
         expires_at = now() + make_interval(days => $3) where id = $1",
    )
    .bind(row.id)
    .bind(hash_token(&next))
    .bind(REFRESH_DAYS as i32)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(tokens_json(&st, &user, row.id, &next)))
}

async fn mark_revoked(st: &Shared, sid: Uuid) -> ApiResult<()> {
    // Los access tokens ya emitidos (15 min) dejan de valer al instante
    let mut r = st.redis.clone();
    let _: () = r
        .set_ex(app_revoked_key(sid), 1, APP_ACCESS_SECS as u64 + 60)
        .await?;
    Ok(())
}

async fn revoke(State(st): State<Shared>, Body(i): Body<RefreshInput>) -> ApiResult<StatusCode> {
    let sid: Option<Uuid> = sqlx::query_scalar(
        "update app_sessions set revoked_at = now() where refresh_hash = $1 and revoked_at is null returning id",
    )
    .bind(hash_token(&i.refresh_token))
    .fetch_optional(&st.db)
    .await?;
    if let Some(sid) = sid {
        mark_revoked(&st, sid).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeInput {
    code: String,
    code_verifier: String,
    device_name: Option<String>,
    platform: Option<String>,
}

/// Código de un solo uso del SSO para apps (`rvmail:appcode:<código>` en Redis).
#[derive(serde::Serialize, Deserialize)]
pub struct AppCode {
    pub user_id: Uuid,
    pub challenge: String,
}

async fn exchange(
    State(st): State<Shared>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Body(i): Body<ExchangeInput>,
) -> ApiResult<Json<Value>> {
    let invalid =
        || ApiError::unauthorized("El código caducó o no es válido; vuelve a iniciar sesión");
    let mut r = st.redis.clone();
    let raw: Option<String> = redis::cmd("GETDEL")
        .arg(format!("rvmail:appcode:{}", i.code))
        .query_async(&mut r)
        .await?;
    let code: AppCode = raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(invalid)?;
    // PKCE: solo la app que inició el flujo conoce el verificador
    if crate::oauth::pkce_challenge(&i.code_verifier) != code.challenge {
        return Err(invalid());
    }
    let user: Option<User> = sqlx::query_as("select * from users where id = $1")
        .bind(code.user_id)
        .fetch_optional(&st.db)
        .await?;
    let user = user.ok_or_else(invalid)?;
    Ok(Json(
        open_session(&st, &user, i.device_name, i.platform, &headers, Some(peer)).await?,
    ))
}

async fn list(State(st): State<Shared>, Session(u): Session) -> ApiResult<Json<Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        device_name: String,
        platform: String,
        ip: Option<String>,
        created_at: DateTime<Utc>,
        last_used_at: DateTime<Utc>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "select id, device_name, platform, ip, created_at, last_used_at from app_sessions
         where user_id = $1 and revoked_at is null and expires_at > now() order by last_used_at desc",
    )
    .bind(u.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!(rows
        .into_iter()
        .map(|r| json!({
            "id": r.id, "deviceName": r.device_name, "platform": r.platform, "ip": r.ip,
            "createdAt": r.created_at, "lastUsedAt": r.last_used_at,
            "current": u.app_session == Some(r.id),
        }))
        .collect::<Vec<_>>())))
}

async fn remove(
    State(st): State<Shared>,
    Session(u): Session,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let done = sqlx::query("update app_sessions set revoked_at = now() where id = $1 and user_id = $2 and revoked_at is null")
        .bind(id)
        .bind(u.id)
        .execute(&st.db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found("Sesión no encontrada"));
    }
    mark_revoked(&st, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
