//! Tokens de API para agentes de IA (MCP) e integraciones. Solo se gestionan
//! desde la sesión del navegador: un token no puede crear otros tokens.

use super::auth::{hash_token, Session, TOKEN_PREFIX};
use super::error::{ApiError, ApiResult};
use super::state::Shared;
use super::validate::{self, Body};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use rvmail_core::models::API_SCOPES;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::types::Json as SqlJson;
use sqlx::FromRow;
use uuid::Uuid;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/tokens", get(list).post(create))
        .route("/tokens/{id}", delete(revoke))
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct TokenDto {
    id: Uuid,
    name: String,
    prefix: String,
    scopes: SqlJson<Vec<String>>,
    account_ids: Option<SqlJson<Vec<Uuid>>>,
    last_used_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

async fn list(State(st): State<Shared>, Session(u): Session) -> ApiResult<Json<Vec<TokenDto>>> {
    Ok(Json(
        sqlx::query_as(
            "select id, name, prefix, scopes, account_ids, last_used_at, expires_at, created_at
             from api_tokens where user_id = $1 and revoked_at is null order by created_at desc",
        )
        .bind(u.id)
        .fetch_all(&st.db)
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInput {
    name: String,
    scopes: Vec<String>,
    account_ids: Option<Vec<Uuid>>,
    expires_in_days: Option<i64>,
}

/// Devuelve el token en claro UNA sola vez; después solo se conserva su hash.
async fn create(
    State(st): State<Shared>,
    Session(u): Session,
    Body(i): Body<CreateInput>,
) -> ApiResult<Json<Value>> {
    let name = i.name.trim().to_string();
    validate::len("name", &name, 1, 80)?;
    if i.scopes.is_empty() {
        return Err(ApiError::bad_request(
            "Datos inválidos — scopes: elige al menos un permiso",
        ));
    }
    if let Some(bad) = i.scopes.iter().find(|s| !API_SCOPES.contains(&s.as_str())) {
        return Err(ApiError::bad_request(format!(
            "Datos inválidos — scopes: permiso desconocido ({bad})"
        )));
    }
    if let Some(d) = i.expires_in_days {
        if !(1..=3650).contains(&d) {
            return Err(ApiError::bad_request(
                "Datos inválidos — expiresInDays: entre 1 y 3650",
            ));
        }
    }
    let mut scopes = i.scopes.clone();
    scopes.dedup();
    let account_ids = i.account_ids.filter(|a| !a.is_empty()).map(|mut a| {
        a.sort();
        a.dedup();
        a
    });
    if let Some(ids) = &account_ids {
        let owned: i64 = sqlx::query_scalar(
            "select count(*) from mail_accounts where user_id = $1 and id = any($2)",
        )
        .bind(u.id)
        .bind(ids)
        .fetch_one(&st.db)
        .await?;
        if owned != ids.len() as i64 {
            return Err(ApiError::bad_request("Alguna de las cuentas no es tuya"));
        }
    }
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(raw));
    let prefix: String = token.chars().take(TOKEN_PREFIX.len() + 6).collect();
    let id: Uuid = sqlx::query_scalar(
        "insert into api_tokens (user_id, name, token_hash, prefix, scopes, account_ids, expires_at)
         values ($1,$2,$3,$4,$5,$6, case when $7::bigint is null then null else now() + make_interval(days => $7::int) end)
         returning id",
    )
    .bind(u.id)
    .bind(&name)
    .bind(hash_token(&token))
    .bind(&prefix)
    .bind(SqlJson(&scopes))
    .bind(account_ids.as_ref().map(SqlJson))
    .bind(i.expires_in_days)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(
        json!({ "id": id, "name": name, "prefix": prefix, "scopes": scopes, "token": token }),
    ))
}

async fn revoke(
    State(st): State<Shared>,
    Session(u): Session,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let r = sqlx::query("update api_tokens set revoked_at = now() where id = $1 and user_id = $2 and revoked_at is null")
        .bind(id)
        .bind(u.id)
        .execute(&st.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::not_found("Token no encontrado"));
    }
    Ok(StatusCode::NO_CONTENT)
}
