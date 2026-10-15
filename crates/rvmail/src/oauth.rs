//! OAuth2 / OpenID Connect con Google, Microsoft y Yahoo. Lo usan la API
//! (inicio de sesión SSO, conectar cuentas) y el worker (IMAP con XOAUTH2).
//!
//! Seguridad:
//! - Flujo de código con `state`, `nonce` y PKCE (S256) cuando el proveedor lo admite.
//! - El `id_token` llega directo del endpoint de tokens por TLS (OIDC Core 3.1.3.7):
//!   se validan `iss`, `aud`, `exp` y `nonce`.
//! - Refresh tokens cifrados en BD (regla 2); access tokens cifrados en Redis
//!   solo mientras son válidos.

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use rvmail_core::crypto::MasterKey;
use rvmail_core::models::MailAccount;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub login_scopes: &'static str,
    pub mail_scopes: &'static str,
    pub imap: (&'static str, u16),
    /// (host, puerto, TLS implícito)
    pub smtp: (&'static str, u16, bool),
    pub pkce: bool,
    /// ¿Se puede confiar en el correo del id_token para vincular a un usuario existente?
    pub trust_email: bool,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

/// Proveedores configurados (con client id y secret).
pub fn providers() -> Vec<Provider> {
    let tenant = Some(env("SSO_MICROSOFT_TENANT"))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "common".into());
    let all = [
        Provider {
            id: "google",
            name: "Google",
            client_id: env("SSO_GOOGLE_CLIENT_ID"),
            client_secret: env("SSO_GOOGLE_CLIENT_SECRET"),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            login_scopes: "openid email profile",
            mail_scopes: "openid email https://mail.google.com/",
            imap: ("imap.gmail.com", 993),
            smtp: ("smtp.gmail.com", 465, true),
            pkce: true,
            trust_email: true,
        },
        Provider {
            id: "microsoft",
            name: "Microsoft",
            client_id: env("SSO_MICROSOFT_CLIENT_ID"),
            client_secret: env("SSO_MICROSOFT_CLIENT_SECRET"),
            auth_url: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
            token_url: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
            login_scopes: "openid email profile",
            mail_scopes: "openid email offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
            imap: ("outlook.office365.com", 993),
            smtp: ("smtp.office365.com", 587, false),
            pkce: true,
            // En cuentas de organización el correo no siempre está verificado ("nOAuth")
            trust_email: false,
        },
        Provider {
            id: "yahoo",
            name: "Yahoo",
            client_id: env("SSO_YAHOO_CLIENT_ID"),
            client_secret: env("SSO_YAHOO_CLIENT_SECRET"),
            auth_url: "https://api.login.yahoo.com/oauth2/request_auth".into(),
            token_url: "https://api.login.yahoo.com/oauth2/get_token".into(),
            login_scopes: "openid email",
            mail_scopes: "openid email mail-w",
            imap: ("imap.mail.yahoo.com", 993),
            smtp: ("smtp.mail.yahoo.com", 465, true),
            pkce: false,
            trust_email: true,
        },
    ];
    #[allow(unused_mut)]
    let mut list: Vec<Provider> = all
        .into_iter()
        .filter(|p| !p.client_id.is_empty() && !p.client_secret.is_empty())
        .collect();
    // Solo en compilaciones de desarrollo: apunta Google a scripts/fake-oidc.mjs para pruebas
    #[cfg(debug_assertions)]
    if let Some(base) = Some(env("RVMAIL_DEV_OIDC_BASE")).filter(|b| !b.is_empty()) {
        for p in list.iter_mut().filter(|p| p.id == "google") {
            p.auth_url = format!("{base}/auth");
            p.token_url = format!("{base}/token");
            p.imap = ("localhost", 3143);
            p.smtp = ("localhost", 3025, false);
        }
    }
    list
}

pub fn provider(id: &str) -> Option<Provider> {
    providers().into_iter().find(|p| p.id == id)
}

pub fn random_token(bytes: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub id_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// El usuario revocó el acceso o el refresh token caducó: hay que reconectar
    #[error("El proveedor revocó el acceso; vuelve a conectar la cuenta")]
    Revoked,
    #[error("{0}")]
    Other(String),
}

async fn token_request(
    http: &reqwest::Client,
    p: &Provider,
    form: &[(&str, &str)],
) -> Result<TokenResponse, OAuthError> {
    let mut f: Vec<(&str, &str)> = form.to_vec();
    f.push(("client_id", &p.client_id));
    f.push(("client_secret", &p.client_secret));
    let res = http
        .post(&p.token_url)
        .form(&f)
        .send()
        .await
        .map_err(|_| OAuthError::Other(format!("No se pudo contactar a {}", p.name)))?;
    let status = res.status();
    let body: Value = res
        .json()
        .await
        .map_err(|_| OAuthError::Other(format!("Respuesta no válida de {}", p.name)))?;
    if !status.is_success() {
        let code = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        tracing::warn!(provider = p.id, %status, error = code, "error del endpoint de tokens");
        if code == "invalid_grant" {
            return Err(OAuthError::Revoked);
        }
        return Err(OAuthError::Other(format!(
            "{} rechazó la solicitud ({code})",
            p.name
        )));
    }
    serde_json::from_value(body)
        .map_err(|_| OAuthError::Other(format!("Respuesta no válida de {}", p.name)))
}

pub async fn exchange_code(
    http: &reqwest::Client,
    p: &Provider,
    code: &str,
    redirect_uri: &str,
    verifier: Option<&str>,
) -> Result<TokenResponse, OAuthError> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(v) = verifier {
        form.push(("code_verifier", v));
    }
    token_request(http, p, &form).await
}

pub async fn refresh(
    http: &reqwest::Client,
    p: &Provider,
    refresh_token: &str,
) -> Result<TokenResponse, OAuthError> {
    token_request(
        http,
        p,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

/// Identidad verificada del usuario según el id_token.
#[derive(Debug, Clone)]
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

pub fn decode_id_token(p: &Provider, id_token: &str, nonce: &str) -> Result<Identity> {
    let payload = id_token.split('.').nth(1).context("id_token mal formado")?;
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))?)?;
    let s = |k: &str| claims.get(k).and_then(Value::as_str).map(str::to_string);

    let iss = s("iss").unwrap_or_default();
    let iss_ok = match p.id {
        "google" => iss == "https://accounts.google.com" || iss == "accounts.google.com",
        "microsoft" => {
            iss.starts_with("https://login.microsoftonline.com/") && iss.ends_with("/v2.0")
        }
        "yahoo" => iss == "https://api.login.yahoo.com",
        _ => false,
    };
    if !iss_ok {
        bail!("emisor del id_token no válido: {iss}");
    }
    let aud_ok = match claims.get("aud") {
        Some(Value::String(a)) => *a == p.client_id,
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(&p.client_id)),
        _ => false,
    };
    if !aud_ok {
        bail!("audiencia del id_token no válida");
    }
    let exp = claims.get("exp").and_then(Value::as_i64).unwrap_or(0);
    if exp + 60 < chrono::Utc::now().timestamp() {
        bail!("id_token caducado");
    }
    if s("nonce").as_deref() != Some(nonce) {
        bail!("nonce del id_token no coincide");
    }
    // Microsoft: tid + oid identifica a la persona de forma estable
    let subject = match (p.id, s("tid"), s("oid")) {
        ("microsoft", Some(tid), Some(oid)) => format!("{tid}:{oid}"),
        _ => s("sub").ok_or_else(|| anyhow!("id_token sin 'sub'"))?,
    };
    let email = s("email")
        .or_else(|| {
            if p.id == "microsoft" {
                s("preferred_username")
            } else {
                None
            }
        })
        .map(|e| e.to_lowercase());
    let email_verified = match claims.get("email_verified") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(v)) => v == "true",
        _ => false,
    };
    Ok(Identity {
        subject,
        email,
        email_verified: email_verified && p.trust_email,
        name: s("name"),
    })
}

fn at_key(account: Uuid) -> String {
    format!("rvmail:oauth:at:{account}")
}

/// Access token vigente para una cuenta OAuth2; lo refresca si hace falta y
/// guarda el refresh token nuevo si el proveedor lo rota.
pub async fn access_token(
    db: &PgPool,
    redis: &ConnectionManager,
    key: &MasterKey,
    http: &reqwest::Client,
    a: &MailAccount,
) -> Result<String, OAuthError> {
    let mut r = redis.clone();
    if let Ok(Some(enc)) = r.get::<_, Option<String>>(at_key(a.id)).await {
        if let Ok(t) = key.decrypt(&enc) {
            return Ok(t);
        }
    }
    let pid = a.oauth_provider.as_deref().unwrap_or_default();
    let p = provider(pid).ok_or_else(|| {
        OAuthError::Other(format!("El proveedor OAuth '{pid}' no está configurado"))
    })?;
    let refresh_token = key
        .decrypt(&a.secret_enc)
        .map_err(|e| OAuthError::Other(e.to_string()))?;
    let t = refresh(http, &p, &refresh_token).await?;
    if let Some(new_rt) = t.refresh_token.as_deref().filter(|rt| *rt != refresh_token) {
        let _ = sqlx::query("update mail_accounts set secret_enc = $2 where id = $1")
            .bind(a.id)
            .bind(key.encrypt(new_rt))
            .execute(db)
            .await;
    }
    let ttl = t.expires_in.unwrap_or(3600).saturating_sub(120).max(60) as u64;
    let _: Result<(), _> = r
        .set_ex(at_key(a.id), key.encrypt(&t.access_token), ttl)
        .await;
    Ok(t.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_provider() -> Provider {
        Provider {
            id: "google",
            name: "Google",
            client_id: "cid".into(),
            client_secret: "s".into(),
            auth_url: String::new(),
            token_url: String::new(),
            login_scopes: "",
            mail_scopes: "",
            imap: ("", 0),
            smtp: ("", 0, true),
            pkce: true,
            trust_email: true,
        }
    }

    fn jwt(claims: Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn validates_id_token() {
        let p = fake_provider();
        let exp = chrono::Utc::now().timestamp() + 600;
        let ok = jwt(
            serde_json::json!({"iss":"https://accounts.google.com","aud":"cid","exp":exp,"nonce":"n1","sub":"123","email":"A@x.com","email_verified":true}),
        );
        let id = decode_id_token(&p, &ok, "n1").unwrap();
        assert_eq!(id.subject, "123");
        assert_eq!(id.email.as_deref(), Some("a@x.com"));
        assert!(id.email_verified);
        assert!(decode_id_token(&p, &ok, "otro").is_err(), "nonce distinto");
        let bad_aud = jwt(
            serde_json::json!({"iss":"https://accounts.google.com","aud":"x","exp":exp,"nonce":"n1","sub":"1"}),
        );
        assert!(decode_id_token(&p, &bad_aud, "n1").is_err());
        let bad_iss = jwt(
            serde_json::json!({"iss":"https://evil.com","aud":"cid","exp":exp,"nonce":"n1","sub":"1"}),
        );
        assert!(decode_id_token(&p, &bad_iss, "n1").is_err());
    }

    #[test]
    fn microsoft_never_trusts_email() {
        let mut p = fake_provider();
        p.id = "microsoft";
        p.trust_email = false;
        let exp = chrono::Utc::now().timestamp() + 600;
        let t = jwt(
            serde_json::json!({"iss":"https://login.microsoftonline.com/t1/v2.0","aud":"cid","exp":exp,"nonce":"n","sub":"s","tid":"t1","oid":"o1","preferred_username":"Ana@corp.com","email_verified":true}),
        );
        let id = decode_id_token(&p, &t, "n").unwrap();
        assert_eq!(id.subject, "t1:o1");
        assert_eq!(id.email.as_deref(), Some("ana@corp.com"));
        assert!(!id.email_verified);
    }

    #[test]
    fn pkce_rfc7636_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
