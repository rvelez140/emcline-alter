//! Autenticación: cookie de sesión (JWT HS256, compatible con la versión Node)
//! para el navegador, y tokens de API `rvm_…` (Bearer) con ámbitos para
//! agentes de IA e integraciones. Mínimo privilegio (regla 11).

use super::error::{ApiError, ApiResult};
use super::state::Shared;
use super::validate::{self, Body};
use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use redis::AsyncCommands;
use rvmail_core::models::User;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "rv_session";
pub const TOKEN_PREFIX: &str = "rvm_";

#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub account_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone)]
pub struct SessionUser {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    /// Presente solo si la petición llegó con un token de API.
    pub token: Option<TokenGrant>,
}

impl SessionUser {
    /// ¿Tiene el ámbito? Las sesiones del navegador lo tienen todo.
    pub fn has(&self, scope: &str) -> bool {
        self.token
            .as_ref()
            .is_none_or(|t| t.scopes.iter().any(|s| s == scope))
    }
    pub fn require(&self, scope: &str) -> ApiResult<()> {
        if self.has(scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "Este token no tiene permiso \"{scope}\""
            )))
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Claims {
    id: Uuid,
    email: String,
    role: String,
    iat: i64,
    exp: i64,
}

pub fn sign_session(st: &Shared, u: &User) -> String {
    let now = Utc::now().timestamp();
    let claims = Claims {
        id: u.id,
        email: u.email.clone(),
        role: u.role.clone(),
        iat: now,
        exp: now + st.cfg.session_days * 86_400,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(st.cfg.jwt_secret.as_bytes()),
    )
    .expect("JWT firmable")
}

pub fn read_session_jar(st: &Shared, jar: &CookieJar) -> Option<SessionUser> {
    let token = jar.get(SESSION_COOKIE)?.value().to_string();
    let data = decode::<Claims>(
        &token,
        &DecodingKey::from_secret(st.cfg.jwt_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .ok()?;
    Some(SessionUser {
        id: data.claims.id,
        email: data.claims.email,
        role: data.claims.role,
        token: None,
    })
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let h = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, rest) = h.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim().to_string())
}

/// Resuelve un token Bearer a su usuario y permisos. None si no existe, expiró o fue revocado.
pub async fn resolve_token(st: &Shared, raw: &str) -> ApiResult<Option<SessionUser>> {
    if !raw.starts_with(TOKEN_PREFIX) {
        return Ok(None);
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        name: String,
        scopes: sqlx::types::Json<Vec<String>>,
        account_ids: Option<sqlx::types::Json<Vec<Uuid>>>,
        expires_at: Option<chrono::DateTime<Utc>>,
        last_used_at: Option<chrono::DateTime<Utc>>,
        user_id: Uuid,
        email: String,
        role: String,
    }
    let row: Option<Row> = sqlx::query_as(
        "select t.id, t.name, t.scopes, t.account_ids, t.expires_at, t.last_used_at, u.id as user_id, u.email, u.role
         from api_tokens t join users u on u.id = t.user_id
         where t.token_hash = $1 and t.revoked_at is null",
    )
    .bind(hash_token(raw))
    .fetch_optional(&st.db)
    .await?;
    let Some(r) = row else { return Ok(None) };
    if r.expires_at.is_some_and(|e| e < Utc::now()) {
        return Ok(None);
    }
    // Último uso: como mucho una escritura por minuto, sin bloquear la petición
    if r.last_used_at
        .is_none_or(|t| Utc::now() - t > chrono::Duration::seconds(60))
    {
        let db = st.db.clone();
        let id = r.id;
        tokio::spawn(async move {
            let _ = sqlx::query("update api_tokens set last_used_at = now() where id = $1")
                .bind(id)
                .execute(&db)
                .await;
        });
    }
    Ok(Some(SessionUser {
        id: r.user_id,
        email: r.email,
        role: r.role,
        token: Some(TokenGrant {
            id: r.id,
            name: r.name,
            scopes: r.scopes.0,
            account_ids: r.account_ids.map(|a| a.0),
        }),
    }))
}

/// Sesión del navegador O token de API. Cada handler exige su ámbito con `user.require(…)`.
pub struct Authed(pub SessionUser);

impl<S> FromRequestParts<S> for Authed
where
    Shared: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let st = Shared::from_ref(state);
        if let Some(raw) = bearer(&parts.headers) {
            return match resolve_token(&st, &raw).await? {
                Some(u) => Ok(Authed(u)),
                None => Err(ApiError::unauthorized(
                    "Token no válido, expirado o revocado",
                )),
            };
        }
        let jar = CookieJar::from_headers(&parts.headers);
        read_session_jar(&st, &jar)
            .map(Authed)
            .ok_or_else(|| ApiError::unauthorized("Sesión no válida o expirada"))
    }
}

/// Solo la sesión del navegador (gestión de cuentas, tokens, administración).
pub struct Session(pub SessionUser);

impl<S> FromRequestParts<S> for Session
where
    Shared: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if bearer(&parts.headers).is_some() {
            return Err(ApiError::forbidden("Este endpoint no admite tokens de API"));
        }
        let Authed(u) = Authed::from_request_parts(parts, state).await?;
        Ok(Session(u))
    }
}

/// Solo administradores de la instalación (nunca con token).
pub struct Admin(pub SessionUser);

impl<S> FromRequestParts<S> for Admin
where
    Shared: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Session(u) = Session::from_request_parts(parts, state).await?;
        if u.role != "admin" {
            return Err(ApiError::forbidden(
                "Solo el administrador puede hacer esto",
            ));
        }
        Ok(Admin(u))
    }
}

pub fn hash_password(pw: &str) -> String {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .expect("argon2")
        .to_string()
}

pub fn verify_password(hash: &str, pw: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(pw.as_bytes(), &h)
            .is_ok()
    })
}

pub fn user_json(u: &User) -> Value {
    json!({ "id": u.id, "name": u.name, "email": u.email, "role": u.role })
}

fn session_cookie(st: &Shared, u: &User) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, sign_session(st, u)))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(st.cfg.secure_cookies)
        .path("/")
        .max_age(time::Duration::days(st.cfg.session_days))
        .build()
}

// ------------------------------------------------------------------ rutas

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/auth/setup", get(setup))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
}

async fn user_count(st: &Shared) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await?)
}

/// Indica si la instalación aún no tiene usuarios (pantalla de configuración inicial).
async fn setup(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    let n = user_count(&st).await?;
    Ok(Json(json!({
        "needsSetup": n == 0,
        "allowSignup": st.cfg.allow_signup,
        "mailboxLogin": st.mailserver.enabled(),
        "sso": super::sso::enabled_providers(&st),
    })))
}

#[derive(Deserialize)]
struct RegisterInput {
    name: String,
    email: String,
    password: String,
}

async fn register(
    State(st): State<Shared>,
    jar: CookieJar,
    Body(i): Body<RegisterInput>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let name = i.name.trim().to_string();
    validate::len("name", &name, 1, 100)?;
    let email = validate::email("email", &i.email)?;
    validate::len("password", &i.password, 10, 200)?;
    let n = user_count(&st).await?;
    if n > 0 && !st.cfg.allow_signup {
        return Err(ApiError::forbidden(
            "El registro está cerrado. Pide acceso al administrador.",
        ));
    }
    let exists: Option<Uuid> = sqlx::query_scalar("select id from users where email = $1")
        .bind(&email)
        .fetch_optional(&st.db)
        .await?;
    if exists.is_some() {
        return Err(ApiError::forbidden("Ya existe un usuario con ese correo"));
    }
    let user: User = sqlx::query_as(
        "insert into users (name, email, password_hash, role) values ($1, $2, $3, $4) returning *",
    )
    .bind(&name)
    .bind(&email)
    .bind(hash_password(&i.password))
    .bind(if n == 0 { "admin" } else { "user" })
    .fetch_one(&st.db)
    .await?;
    Ok((jar.add(session_cookie(&st, &user)), Json(user_json(&user))))
}

#[derive(Deserialize)]
struct LoginInput {
    email: String,
    password: String,
}

fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    // Detrás del proxy (nginx/Caddy), el primer X-Forwarded-For es el cliente
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| peer.map(|p| p.ip().to_string()))
        .unwrap_or_default()
}

async fn login(
    State(st): State<Shared>,
    headers: HeaderMap,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    jar: CookieJar,
    Body(i): Body<LoginInput>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let email = validate::email("email", &i.email)?;
    validate::len("password", &i.password, 1, 200)?;
    let rl_key = format!(
        "rvmail:rl:login:{}:{}",
        client_ip(&headers, Some(peer)),
        email
    );
    let mut r = st.redis.clone();
    let attempts: i64 = r.incr(&rl_key, 1).await?;
    if attempts == 1 {
        let _: bool = r.expire(&rl_key, 15 * 60).await?;
    }
    if attempts > 10 {
        return Err(ApiError::too_many(
            "Demasiados intentos. Espera 15 minutos.",
        ));
    }

    let existing: Option<User> = sqlx::query_as("select * from users where email = $1")
        .bind(&email)
        .fetch_optional(&st.db)
        .await?;
    let user = match existing {
        Some(u)
            if u.password_hash
                .as_deref()
                .is_some_and(|h| verify_password(h, &i.password)) =>
        {
            u
        }
        other => super::mailserver::mailbox_login(&st, &email, &i.password, other)
            .await?
            .ok_or_else(|| ApiError::unauthorized("Correo o contraseña incorrectos"))?,
    };
    let _: i64 = r.del(&rl_key).await?;
    Ok((jar.add(session_cookie(&st, &user)), Json(user_json(&user))))
}

async fn logout(jar: CookieJar) -> (axum::http::StatusCode, CookieJar) {
    (
        axum::http::StatusCode::NO_CONTENT,
        jar.remove(Cookie::build(SESSION_COOKIE).path("/")),
    )
}

async fn me(State(st): State<Shared>, Session(s): Session) -> ApiResult<Json<Value>> {
    let u: Option<User> = sqlx::query_as("select * from users where id = $1")
        .bind(s.id)
        .fetch_optional(&st.db)
        .await?;
    u.map(|u| Json(user_json(&u)))
        .ok_or_else(|| ApiError::unauthorized("Sesión no válida o expirada"))
}

/// Inicia sesión a un usuario ya verificado (SSO): devuelve la cookie.
pub fn login_cookie(st: &Shared, jar: CookieJar, u: &User) -> CookieJar {
    jar.add(session_cookie(st, u))
}
