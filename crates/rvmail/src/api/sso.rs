//! Inicio de sesión con Google, Microsoft y Yahoo (OpenID Connect) y conexión
//! de buzones por OAuth2 (XOAUTH2) sin contraseñas de aplicación.
//!
//! Modos del flujo (`/api/auth/sso/{proveedor}/start?mode=…`):
//!  - `login`   entrar a RV Mail
//!  - `link`    vincular la identidad al usuario con sesión iniciada
//!  - `account` conectar el buzón (Gmail / Outlook / Yahoo) como cuenta de correo

use super::accounts::{dto, notify};
use super::auth::{login_cookie, Session};
use super::error::{ApiError, ApiResult};
use super::state::Shared;
use crate::oauth::{self, Identity, OAuthError, Provider};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use redis::AsyncCommands;
use rvmail_core::events::WorkerControl;
use rvmail_core::models::{MailAccount, User};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

const STATE_TTL: u64 = 600;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/auth/sso/{provider}/start", get(start))
        .route("/auth/sso/{provider}/callback", get(callback))
        .route("/auth/identities", get(identities))
        .route("/auth/identities/{id}", delete(unlink))
        .route("/oauth/providers", get(mail_providers))
}

/// Proveedores de inicio de sesión configurados (para los botones del login).
pub fn enabled_providers(_st: &Shared) -> Vec<Value> {
    oauth::providers()
        .iter()
        .map(|p| json!({ "id": p.id, "name": p.name }))
        .collect()
}

/// Access token vigente para una cuenta OAuth2 (refresca si hace falta).
pub async fn access_token_for(st: &Shared, a: &MailAccount) -> ApiResult<String> {
    match oauth::access_token(&st.db, &st.redis, &st.core.master_key, &st.http, a).await {
        Ok(t) => Ok(t),
        Err(OAuthError::Revoked) => {
            let _ = sqlx::query(
                "update mail_accounts set status = 'auth_error', last_error = $2 where id = $1",
            )
            .bind(a.id)
            .bind("El proveedor revocó el acceso; vuelve a conectar la cuenta")
            .execute(&st.db)
            .await;
            Err(ApiError::bad_gateway(
                "El proveedor revocó el acceso; vuelve a conectar la cuenta",
            ))
        }
        Err(OAuthError::Other(m)) => Err(ApiError::bad_gateway(m)),
    }
}

#[derive(Serialize, Deserialize)]
struct FlowState {
    provider: String,
    mode: String,
    nonce: String,
    verifier: Option<String>,
    user_id: Option<Uuid>,
    /// Inicio de sesión desde una app nativa: adónde volver y reto PKCE de la app
    #[serde(default)]
    app_redirect: Option<String>,
    #[serde(default)]
    app_challenge: Option<String>,
}

fn redirect_uri(st: &Shared, p: &Provider) -> String {
    format!(
        "{}/api/auth/sso/{}/callback",
        st.cfg.app_url.trim_end_matches('/'),
        p.id
    )
}

fn back(path_and_query: &str) -> Response {
    Redirect::to(path_and_query).into_response()
}

fn fail(msg: &str) -> Response {
    let q: String = url_escape(msg);
    back(&format!("/?sso_error={q}"))
}

fn url_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
struct StartQuery {
    mode: Option<String>,
    /// Apps nativas (RFC 8252): `rvmail://…` o `http://127.0.0.1:<puerto>/…`
    app_redirect: Option<String>,
    /// Reto PKCE S256 de la app para canjear el código en /auth/token/exchange
    app_challenge: Option<String>,
}

/// Solo se vuelve a la app propia o a un puerto local (nunca a otra web).
pub fn valid_app_redirect(u: &str) -> bool {
    let loopback = ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|p| u.starts_with(p));
    (u.starts_with("rvmail://") || loopback) && u.len() <= 300 && !u.contains(['#', ' ', '\n'])
}

/// Es una navegación del navegador: los errores vuelven a la web como `?sso_error=`.
async fn start(
    State(st): State<Shared>,
    jar: CookieJar,
    Path(pid): Path<String>,
    Query(q): Query<StartQuery>,
) -> Response {
    match start_flow(&st, &jar, &pid, q).await {
        Ok(r) => r,
        Err(e) => fail(&e.message),
    }
}

async fn start_flow(st: &Shared, jar: &CookieJar, pid: &str, q: StartQuery) -> ApiResult<Response> {
    let p = oauth::provider(pid).ok_or_else(|| ApiError::not_found("Proveedor no configurado"))?;
    let mode = q.mode.unwrap_or_else(|| "login".into());
    let user_id = match mode.as_str() {
        "login" => None,
        "link" | "account" => {
            // Requiere sesión del navegador (la cookie viaja en la navegación de primer nivel)
            let u = super::auth::read_session_jar(st, jar)
                .ok_or_else(|| ApiError::unauthorized("Inicia sesión primero"))?;
            Some(u.id)
        }
        _ => return Err(ApiError::bad_request("Datos inválidos — mode")),
    };
    let (app_redirect, app_challenge) = match (q.app_redirect, q.app_challenge) {
        (None, _) => (None, None),
        (Some(r), Some(c)) if mode == "login" && valid_app_redirect(&r) && c.len() == 43 => {
            (Some(r), Some(c))
        }
        _ => return Err(ApiError::bad_request("Datos inválidos — app_redirect")),
    };
    let state = oauth::random_token(24);
    let nonce = oauth::random_token(16);
    let verifier = p.pkce.then(|| oauth::random_token(48));
    let flow = FlowState {
        provider: p.id.into(),
        mode: mode.clone(),
        nonce: nonce.clone(),
        verifier: verifier.clone(),
        user_id,
        app_redirect,
        app_challenge,
    };
    let mut r = st.redis.clone();
    let _: () = r
        .set_ex(
            format!("rvmail:sso:state:{state}"),
            serde_json::to_string(&flow)?,
            STATE_TTL,
        )
        .await?;

    let scopes = if mode == "account" {
        p.mail_scopes
    } else {
        p.login_scopes
    };
    let mut params: Vec<(&str, String)> = vec![
        ("client_id", p.client_id.clone()),
        ("response_type", "code".into()),
        ("redirect_uri", redirect_uri(st, &p)),
        ("scope", scopes.into()),
        ("state", state),
        ("nonce", nonce),
    ];
    if let Some(v) = &verifier {
        params.push(("code_challenge", oauth::pkce_challenge(v)));
        params.push(("code_challenge_method", "S256".into()));
    }
    if mode == "account" {
        // Acceso sin conexión (refresh token) para que el worker sincronice siempre
        if p.id == "google" {
            params.push(("access_type", "offline".into()));
            params.push(("prompt", "consent".into()));
        }
    } else if p.id == "google" || p.id == "microsoft" {
        params.push(("prompt", "select_account".into()));
    }
    let qs: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", url_escape(v)))
        .collect();
    Ok(back(&format!("{}?{}", p.auth_url, qs.join("&"))))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(st): State<Shared>,
    jar: CookieJar,
    Path(pid): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    match handle_callback(&st, jar, &pid, q).await {
        Ok(r) => r,
        Err(e) => fail(&e.message),
    }
}

async fn handle_callback(
    st: &Shared,
    jar: CookieJar,
    pid: &str,
    q: CallbackQuery,
) -> ApiResult<Response> {
    if let Some(e) = q.error {
        return Err(ApiError::bad_request(if e == "access_denied" {
            "Cancelaste el inicio de sesión".into()
        } else {
            format!("El proveedor devolvió un error ({e})")
        }));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Err(ApiError::bad_request("Respuesta incompleta del proveedor"));
    };
    let mut r = st.redis.clone();
    let raw: Option<String> = redis::cmd("GETDEL")
        .arg(format!("rvmail:sso:state:{state}"))
        .query_async(&mut r)
        .await?;
    let flow: FlowState = raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| {
            ApiError::bad_request("La solicitud de inicio de sesión caducó; inténtalo otra vez")
        })?;
    if flow.provider != pid {
        return Err(ApiError::bad_request("Proveedor inesperado"));
    }
    let p = oauth::provider(pid).ok_or_else(|| ApiError::not_found("Proveedor no configurado"))?;
    let tokens = oauth::exchange_code(
        &st.http,
        &p,
        &code,
        &redirect_uri(st, &p),
        flow.verifier.as_deref(),
    )
    .await
    .map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    let id_token = tokens
        .id_token
        .as_deref()
        .ok_or_else(|| ApiError::bad_gateway("El proveedor no devolvió identidad (id_token)"))?;
    let ident = oauth::decode_id_token(&p, id_token, &flow.nonce).map_err(|e| {
        tracing::warn!(provider = p.id, err = %e, "id_token rechazado");
        ApiError::bad_gateway("No se pudo verificar la identidad devuelta por el proveedor")
    })?;

    match flow.mode.as_str() {
        "login" => {
            let user = login_user(st, &p, &ident).await?;
            if let (Some(to), Some(challenge)) = (flow.app_redirect, flow.app_challenge) {
                // App nativa: código de un solo uso que solo ella puede canjear (PKCE)
                let code = oauth::random_token(32);
                let payload = super::app_auth::AppCode {
                    user_id: user.id,
                    challenge,
                };
                let _: () = r
                    .set_ex(
                        format!("rvmail:appcode:{code}"),
                        serde_json::to_string(&payload)?,
                        super::app_auth::APP_CODE_TTL,
                    )
                    .await?;
                let sep = if to.contains('?') { '&' } else { '?' };
                return Ok(Redirect::to(&format!("{to}{sep}code={code}")).into_response());
            }
            Ok((login_cookie(st, jar, &user), Redirect::to("/")).into_response())
        }
        "link" => {
            let uid = flow
                .user_id
                .ok_or_else(|| ApiError::unauthorized("Inicia sesión primero"))?;
            link_identity(st, uid, &p, &ident).await?;
            Ok(back("/?sso=linked"))
        }
        "account" => {
            let uid = flow
                .user_id
                .ok_or_else(|| ApiError::unauthorized("Inicia sesión primero"))?;
            let rt = tokens.refresh_token.as_deref().ok_or_else(|| {
                ApiError::bad_gateway(
                    "El proveedor no concedió acceso sin conexión; vuelve a intentarlo",
                )
            })?;
            add_oauth_account(
                st,
                uid,
                &p,
                &ident,
                rt,
                &tokens.access_token,
                tokens.expires_in,
            )
            .await?;
            Ok(back("/?account=added"))
        }
        _ => Err(ApiError::bad_request("Modo no válido")),
    }
}

async fn login_user(st: &Shared, p: &Provider, ident: &Identity) -> ApiResult<User> {
    // 1. Identidad ya vinculada
    let linked: Option<User> = sqlx::query_as(
        "select u.* from user_identities i join users u on u.id = i.user_id where i.provider = $1 and i.subject = $2",
    )
    .bind(p.id)
    .bind(&ident.subject)
    .fetch_optional(&st.db)
    .await?;
    if let Some(u) = linked {
        sqlx::query("update user_identities set last_login_at = now(), email = $3 where provider = $1 and subject = $2")
            .bind(p.id)
            .bind(&ident.subject)
            .bind(&ident.email)
            .execute(&st.db)
            .await?;
        return Ok(u);
    }
    let email = ident
        .email
        .clone()
        .ok_or_else(|| ApiError::forbidden("El proveedor no compartió tu correo"))?;
    let existing: Option<User> = sqlx::query_as("select * from users where email = $1")
        .bind(&email)
        .fetch_optional(&st.db)
        .await?;
    let user = match existing {
        // 2. Mismo correo, verificado por un proveedor de confianza → se vincula
        Some(u) if ident.email_verified => u,
        Some(_) => {
            return Err(ApiError::forbidden(format!(
                "Ya existe una cuenta con {email}. Entra con tu contraseña y vincula {} desde Configuración.",
                p.name
            )))
        }
        // 3. Usuario nuevo (solo si el administrador lo permite)
        None if std::env::var("SSO_AUTO_SIGNUP").as_deref() == Ok("true") => {
            let n: i64 = sqlx::query_scalar("select count(*) from users").fetch_one(&st.db).await?;
            if n == 0 {
                return Err(ApiError::forbidden("Primero crea la cuenta de administrador"));
            }
            let name = ident.name.clone().unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_string());
            sqlx::query_as("insert into users (email, name, password_hash, role) values ($1, $2, null, 'user') returning *")
                .bind(&email)
                .bind(name)
                .fetch_one(&st.db)
                .await?
        }
        None => {
            return Err(ApiError::forbidden(format!("No hay ninguna cuenta de RV Mail para {email}. Pide acceso al administrador.")))
        }
    };
    sqlx::query(
        "insert into user_identities (user_id, provider, subject, email, last_login_at) values ($1,$2,$3,$4, now())
         on conflict (provider, subject) do nothing",
    )
    .bind(user.id)
    .bind(p.id)
    .bind(&ident.subject)
    .bind(&ident.email)
    .execute(&st.db)
    .await?;
    Ok(user)
}

async fn link_identity(
    st: &Shared,
    user_id: Uuid,
    p: &Provider,
    ident: &Identity,
) -> ApiResult<()> {
    let owner: Option<Uuid> = sqlx::query_scalar(
        "select user_id from user_identities where provider = $1 and subject = $2",
    )
    .bind(p.id)
    .bind(&ident.subject)
    .fetch_optional(&st.db)
    .await?;
    match owner {
        Some(o) if o != user_id => Err(ApiError::conflict(format!(
            "Esa cuenta de {} ya está vinculada a otro usuario",
            p.name
        ))),
        Some(_) => Ok(()),
        None => {
            sqlx::query("insert into user_identities (user_id, provider, subject, email) values ($1,$2,$3,$4)")
                .bind(user_id)
                .bind(p.id)
                .bind(&ident.subject)
                .bind(&ident.email)
                .execute(&st.db)
                .await?;
            Ok(())
        }
    }
}

async fn add_oauth_account(
    st: &Shared,
    user_id: Uuid,
    p: &Provider,
    ident: &Identity,
    refresh_token: &str,
    access_token: &str,
    expires_in: Option<i64>,
) -> ApiResult<()> {
    let email = ident
        .email
        .clone()
        .ok_or_else(|| ApiError::bad_gateway("El proveedor no compartió la dirección del buzón"))?;
    let enc = st.core.master_key.encrypt(refresh_token);
    let existing: Option<Uuid> =
        sqlx::query_scalar("select id from mail_accounts where user_id = $1 and email = $2")
            .bind(user_id)
            .bind(&email)
            .fetch_optional(&st.db)
            .await?;
    // Si el buzón ya estaba agregado (p. ej. con contraseña de aplicación), pasa a OAuth
    let mut tx = st.db.begin().await?;
    let account_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "insert into mail_accounts (user_id, display_name, email, username, secret_enc, imap_host, smtp_host)
                 values ($1, $2, $3, $3, '', '', '') returning id",
            )
            .bind(user_id)
            .bind(ident.name.clone().unwrap_or_else(|| email.clone()))
            .bind(&email)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    let row: MailAccount = sqlx::query_as(
        "update mail_accounts set auth_type = 'oauth2', oauth_provider = $2, secret_enc = $3, username = $4,
           imap_host = $5, imap_port = $6, imap_secure = ($6 = 993), smtp_host = $7, smtp_port = $8, smtp_secure = $9,
           source = 'external', status = 'pending', last_error = null, enabled = true
         where id = $1 returning *",
    )
    .bind(account_id)
    .bind(p.id)
    .bind(&enc)
    .bind(&email)
    .bind(p.imap.0)
    .bind(i32::from(p.imap.1))
    .bind(p.smtp.0)
    .bind(i32::from(p.smtp.1))
    .bind(p.smtp.2)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    // El access token recién emitido se aprovecha (cifrado) para no refrescar enseguida
    let ttl = expires_in.unwrap_or(3600).saturating_sub(120).max(60) as u64;
    let mut r = st.redis.clone();
    let _: Result<(), _> = r
        .set_ex(
            format!("rvmail:oauth:at:{}", row.id),
            st.core.master_key.encrypt(access_token),
            ttl,
        )
        .await;
    notify(st, WorkerControl::AccountChanged { account_id: row.id }).await;
    tracing::info!(provider = p.id, account = %dto(&row)["email"], "cuenta OAuth conectada");
    Ok(())
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct IdentityDto {
    id: Uuid,
    provider: String,
    email: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_login_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn identities(State(st): State<Shared>, Session(u): Session) -> ApiResult<Json<Value>> {
    let rows: Vec<IdentityDto> = sqlx::query_as(
        "select id, provider, email, created_at, last_login_at from user_identities where user_id = $1 order by created_at",
    )
    .bind(u.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        json!({ "identities": rows, "providers": enabled_providers(&st) }),
    ))
}

async fn unlink(
    State(st): State<Shared>,
    Session(u): Session,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    // No dejar al usuario sin forma de entrar
    let has_password: bool =
        sqlx::query_scalar("select password_hash is not null from users where id = $1")
            .bind(u.id)
            .fetch_one(&st.db)
            .await?;
    let count: i64 = sqlx::query_scalar("select count(*) from user_identities where user_id = $1")
        .bind(u.id)
        .fetch_one(&st.db)
        .await?;
    if !has_password && count <= 1 {
        return Err(ApiError::bad_request(
            "Es tu única forma de entrar: define una contraseña antes de desvincularla",
        ));
    }
    let r = sqlx::query("delete from user_identities where id = $1 and user_id = $2")
        .bind(id)
        .bind(u.id)
        .execute(&st.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::not_found("Vinculación no encontrada"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Proveedores con los que se puede conectar un buzón por OAuth.
async fn mail_providers(Session(_): Session) -> Json<Value> {
    Json(json!(oauth::providers()
        .iter()
        .map(|p| json!({ "id": p.id, "name": p.name }))
        .collect::<Vec<_>>()))
}

#[cfg(test)]
mod app_redirect_tests {
    use super::valid_app_redirect;

    #[test]
    fn only_app_or_loopback() {
        assert!(valid_app_redirect("rvmail://auth"));
        assert!(valid_app_redirect("http://127.0.0.1:43123/cb"));
        assert!(!valid_app_redirect("https://evil.example/cb"));
        assert!(!valid_app_redirect("http://127.0.0.1.evil.com/cb"));
        assert!(!valid_app_redirect("javascript:alert(1)"));
    }
}
