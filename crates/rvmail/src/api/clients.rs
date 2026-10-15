//! Conexiones cortas IMAP/SMTP bajo demanda (regla 4: solo el worker mantiene
//! conexiones persistentes). Resuelve la credencial de la cuenta: contraseña
//! cifrada o, en cuentas OAuth (fase S), un access token fresco.

use super::error::{ApiError, ApiResult};
use super::state::Shared;
use crate::net::imap::{self as imapnet, ImapError, ImapParams, ImapSession};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use rvmail_core::models::MailAccount;
use std::time::Duration;

/// Datos de conexión de una cuenta (o de una prueba antes de guardarla).
#[derive(Clone, Debug)]
pub struct Conn {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_secure: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_secure: bool,
    pub username: String,
    pub secret: Secret,
}

#[derive(Clone)]
pub enum Secret {
    Password(String),
    /// Access token OAuth2 (XOAUTH2)
    OAuth(String),
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)") // nunca registrar credenciales (regla 2)
    }
}

pub async fn conn_for(st: &Shared, a: &MailAccount) -> ApiResult<Conn> {
    let secret = if a.auth_type == "oauth2" {
        Secret::OAuth(super::sso::access_token_for(st, a).await?)
    } else {
        Secret::Password(st.core.master_key.decrypt(&a.secret_enc)?)
    };
    Ok(Conn {
        imap_host: a.imap_host.clone(),
        imap_port: a.imap_port as u16,
        imap_secure: a.imap_secure,
        smtp_host: a.smtp_host.clone(),
        smtp_port: a.smtp_port as u16,
        smtp_secure: a.smtp_secure,
        username: a.username.clone(),
        secret,
    })
}

pub async fn imap(st: &Shared, c: &Conn) -> Result<ImapSession, ImapError> {
    match &c.secret {
        Secret::Password(p) => {
            imapnet::connect(&ImapParams {
                host: &c.imap_host,
                port: c.imap_port,
                secure: c.imap_secure,
                username: &c.username,
                password: p,
                allow_self_signed: st.core.allow_self_signed,
            })
            .await
        }
        Secret::OAuth(token) => {
            imapnet::connect_xoauth2(
                &c.imap_host,
                c.imap_port,
                &c.username,
                token,
                st.core.allow_self_signed,
            )
            .await
        }
    }
}

/// Abre una sesión IMAP corta (cerrarla con `close`).
pub async fn open(st: &Shared, c: &Conn) -> ApiResult<ImapSession> {
    imap(st, c).await.map_err(|e| match e {
        ImapError::Auth => ApiError::bad_gateway("El servidor rechazó usuario o contraseña"),
        ImapError::Other(m) => ApiError::bad_gateway(m),
    })
}

pub async fn close(mut s: ImapSession) {
    let _ = tokio::time::timeout(Duration::from_secs(5), s.logout()).await;
}

/// Error de una operación IMAP → 502 con mensaje entendible; detalle al log.
pub fn imap_fail(e: impl Into<anyhow::Error>) -> ApiError {
    let e = e.into();
    tracing::warn!(err = %format!("{e:#}"), "operación IMAP falló");
    ApiError::bad_gateway(imapnet::humanize(&e))
}

pub fn smtp(st: &Shared, c: &Conn) -> ApiResult<AsyncSmtpTransport<Tokio1Executor>> {
    let tls_params = TlsParameters::builder(c.smtp_host.clone())
        .dangerous_accept_invalid_certs(st.core.allow_self_signed)
        .build_rustls()
        .map_err(|_| ApiError::bad_request("Nombre de servidor SMTP no válido"))?;
    // 465 = TLS implícito; otros puertos = STARTTLS si el servidor lo ofrece
    let tls = if c.smtp_secure {
        Tls::Wrapper(tls_params)
    } else {
        Tls::Opportunistic(tls_params)
    };
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(c.smtp_host.clone())
        .port(c.smtp_port)
        .tls(tls)
        .timeout(Some(Duration::from_secs(20)));
    b = match &c.secret {
        Secret::Password(p) => b.credentials(Credentials::new(c.username.clone(), p.clone())),
        Secret::OAuth(t) => b
            .credentials(Credentials::new(c.username.clone(), t.clone()))
            .authentication(vec![Mechanism::Xoauth2]),
    };
    Ok(b.build())
}

/// Traduce errores SMTP a mensajes entendibles (regla 9).
pub fn humanize_smtp(e: &lettre::transport::smtp::Error) -> String {
    let s = e.to_string();
    if e.status().is_some_and(|c| c.to_string().starts_with("535"))
        || s.contains("authentication")
        || s.contains("535")
    {
        "Usuario o contraseña rechazados por el servidor".into()
    } else if s.contains("refused") {
        "El servidor rechazó la conexión (revisa el puerto)".into()
    } else if s.contains("lookup") || s.contains("resolve") {
        "No se encontró el servidor (revisa el nombre del host)".into()
    } else if s.contains("timed out") || s.contains("timeout") {
        "Tiempo de espera agotado al conectar".into()
    } else if s.to_lowercase().contains("tls") || s.contains("certificate") {
        "Error de TLS/SSL: revisa si el puerto usa conexión segura".into()
    } else if e.is_permanent() {
        format!(
            "El servidor rechazó el envío ({})",
            e.status().map(|c| c.to_string()).unwrap_or_default()
        )
    } else {
        "Error de comunicación con el servidor SMTP".into()
    }
}
