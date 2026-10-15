//! Conexión IMAP: TLS implícito (993), STARTTLS cuando el servidor lo ofrece,
//! o texto plano (solo desarrollo, p. ej. GreenMail en 3143).

use super::tls;
use anyhow::{anyhow, Context, Result};
use async_imap::{Client, Session};
use std::fmt::Debug;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send + Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Debug> Stream for T {}

pub type ImapSession = Session<Box<dyn Stream>>;

pub struct ImapParams<'a> {
    pub host: &'a str,
    pub port: u16,
    pub secure: bool,
    pub username: &'a str,
    pub password: &'a str,
    pub allow_self_signed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    #[error("El servidor rechazó usuario o contraseña")]
    Auth,
    #[error("{0}")]
    Other(String),
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Conecta e inicia sesión. Distingue errores de autenticación (no reintentar).
pub async fn connect(p: &ImapParams<'_>) -> Result<ImapSession, ImapError> {
    let client = open(p).await.map_err(|e| ImapError::Other(humanize(&e)))?;
    match timeout(CONNECT_TIMEOUT, client.login(p.username, p.password)).await {
        Err(_) => Err(ImapError::Other(
            "Tiempo de espera agotado al iniciar sesión".into(),
        )),
        Ok(Ok(s)) => Ok(s),
        Ok(Err((async_imap::error::Error::No(_), _))) => Err(ImapError::Auth),
        Ok(Err((e, _))) => Err(ImapError::Other(format!("Error al iniciar sesión: {e}"))),
    }
}

/// Inicio de sesión con OAuth2 (Gmail, Outlook/M365, Yahoo): mecanismo XOAUTH2.
pub async fn connect_xoauth2(
    host: &str,
    port: u16,
    user: &str,
    access_token: &str,
    allow_self_signed: bool,
) -> Result<ImapSession, ImapError> {
    let p = ImapParams {
        host,
        port,
        secure: true,
        username: user,
        password: "",
        allow_self_signed,
    };
    let client = open(&p).await.map_err(|e| ImapError::Other(humanize(&e)))?;
    let auth = XOAuth2 {
        user: user.to_string(),
        token: access_token.to_string(),
    };
    match timeout(CONNECT_TIMEOUT, client.authenticate("XOAUTH2", auth)).await {
        Err(_) => Err(ImapError::Other(
            "Tiempo de espera agotado al iniciar sesión".into(),
        )),
        Ok(Ok(s)) => Ok(s),
        Ok(Err((async_imap::error::Error::No(_), _))) => Err(ImapError::Auth),
        Ok(Err((e, _))) => Err(ImapError::Other(format!("Error al iniciar sesión: {e}"))),
    }
}

struct XOAuth2 {
    user: String,
    token: String,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}

async fn open(p: &ImapParams<'_>) -> Result<Client<Box<dyn Stream>>> {
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect((p.host, p.port)))
        .await
        .map_err(|_| anyhow!("ETIMEDOUT"))??;
    tcp.set_nodelay(true).ok();

    if p.secure {
        let tls = timeout(
            CONNECT_TIMEOUT,
            tls::connector(p.allow_self_signed).connect(tls::server_name(p.host)?, tcp),
        )
        .await
        .map_err(|_| anyhow!("ETIMEDOUT"))??;
        let mut client = Client::new(Box::new(tls) as Box<dyn Stream>);
        greeting(&mut client).await?;
        return Ok(client);
    }

    // Puerto sin TLS: STARTTLS si el servidor lo admite; si no, texto plano
    let mut plain = Client::new(tcp);
    greeting(&mut plain).await?;
    if plain
        .run_command_and_check_ok("STARTTLS", None)
        .await
        .is_ok()
    {
        let tcp = plain.into_inner();
        let tls = tls::connector(p.allow_self_signed)
            .connect(tls::server_name(p.host)?, tcp)
            .await
            .context("STARTTLS")?;
        return Ok(Client::new(Box::new(tls) as Box<dyn Stream>));
    }
    Ok(Client::new(Box::new(plain.into_inner()) as Box<dyn Stream>))
}

async fn greeting<T: Stream>(c: &mut Client<T>) -> Result<()> {
    timeout(CONNECT_TIMEOUT, c.read_response())
        .await
        .map_err(|_| anyhow!("ETIMEDOUT"))??
        .ok_or_else(|| anyhow!("El servidor cerró la conexión"))?;
    Ok(())
}

/// Traduce errores técnicos a mensajes entendibles (regla 9).
pub fn humanize(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    if s.contains("ETIMEDOUT") || s.contains("timed out") {
        "Tiempo de espera agotado al conectar".into()
    } else if s.contains("refused") {
        "El servidor rechazó la conexión (revisa el puerto)".into()
    } else if s.contains("failed to lookup")
        || s.contains("Name or service not known")
        || s.contains("nodename")
    {
        "No se encontró el servidor (revisa el nombre del host)".into()
    } else if s.contains("certificate")
        || s.contains("Tls")
        || s.contains("TLS")
        || s.contains("handshake")
    {
        "Error de TLS/SSL: revisa si el puerto usa conexión segura o si el certificado es válido"
            .into()
    } else if s.contains("during parsing") || s.contains("Parse") {
        "El servidor de correo envió una respuesta no válida".into()
    } else if s.contains("closed")
        || s.contains("reset")
        || s.contains("broken pipe")
        || s.contains("ConnectionLost")
    {
        "Se perdió la conexión con el servidor de correo".into()
    } else {
        // El detalle completo va al log; al usuario, un mensaje corto
        "Error de comunicación con el servidor de correo".into()
    }
}
