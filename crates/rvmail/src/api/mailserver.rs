//! Servidor de correo integrado (regla 10: capa extra; con `none` RV Mail es
//! solo cliente). Proveedores: Mail Server de aaPanel (Postfix + Dovecot) y,
//! en la fase J, Stalwart. Los buzones alojados se usan como cuentas normales
//! (`mail_accounts.source = 'mailserver'`) y pueden iniciar sesión en RV Mail.

use super::accounts::notify;
use super::auth::{hash_password, Admin, Session};
use super::error::{ApiError, ApiResult};
use super::state::Shared;
use super::validate::{self, Body};
use crate::net::imap::{self as imapnet, ImapParams};
use crate::net::tls;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use hickory_resolver::proto::rr::RData;
use hickory_resolver::TokioResolver;
use md5::{Digest, Md5};
use rand::RngCore;
use redis::AsyncCommands;
use rvmail_core::config::MailServerConfig;
use rvmail_core::events::WorkerControl;
use rvmail_core::models::User;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

const DOMAINS_CACHE: &str = "rvmail:mailserver:domains";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MailDomain {
    pub domain: String,
    pub mailboxes: Option<i64>,
    pub quota_gb: Option<f64>,
    pub created_at: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub email: String,
    pub full_name: String,
    pub quota_gb: Option<f64>,
    pub active: bool,
    pub created_at: Option<String>,
}

#[derive(Default)]
pub struct MailboxPatch {
    pub password: Option<String>,
    pub full_name: Option<String>,
    pub quota_gb: Option<f64>,
    pub active: Option<bool>,
}

// ------------------------------------------------------------------ aaPanel

/// Cliente de la API de aaPanel (Settings → API interface).
/// Firma: request_time = timestamp; request_token = md5(request_time + md5(api_sk)).
/// La IP de la API de RV Mail debe estar en la lista blanca del panel.
pub struct AaPanel {
    base: String,
    key: String,
    http: reqwest::Client,
}

fn md5_hex(s: &str) -> String {
    hex::encode(Md5::digest(s.as_bytes()))
}

impl AaPanel {
    fn new(c: &MailServerConfig) -> Self {
        Self {
            base: c.aapanel_url.clone(),
            key: c.aapanel_api_key.clone(),
            http: tls::http_client(!c.aapanel_verify_tls),
        }
    }

    /// Funciones del plugin "mail_sys". Si tu versión usa otros nombres, compruébalos con
    /// `grep -n "def " /www/server/panel/plugin/mail_sys/mail_sys_main.py`.
    async fn plugin(&self, func: &str, params: &[(&str, String)]) -> Result<Value, String> {
        let t = chrono::Utc::now().timestamp().to_string();
        let mut form: Vec<(&str, String)> = vec![
            ("request_time", t.clone()),
            (
                "request_token",
                md5_hex(&format!("{t}{}", md5_hex(&self.key))),
            ),
        ];
        form.extend(params.iter().cloned());
        let url = format!("{}/plugin?action=a&name=mail_sys&s={func}", self.base);
        let res = self.http.post(&url).form(&form).send().await.map_err(|e| {
            if e.is_timeout() {
                "El panel no respondió a tiempo".to_string()
            } else if e.is_connect() {
                "No se pudo conectar al panel (revisa AAPANEL_URL y el puerto)".to_string()
            } else {
                "Error al contactar el panel".to_string()
            }
        })?;
        let status = res.status();
        let data: Value = res
            .json()
            .await
            .map_err(|_| format!("Respuesta no válida del panel (HTTP {status})"))?;
        if data.get("status") == Some(&Value::Bool(false)) {
            return Err(data
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("El panel rechazó la operación")
                .to_string());
        }
        Ok(data)
    }
}

/// aaPanel devuelve listas en distintas formas según la versión.
fn list_from(v: &Value) -> Vec<Value> {
    if let Some(a) = v.as_array() {
        return a.clone();
    }
    for p in ["/data", "/data/data", "/msg"] {
        if let Some(a) = v.pointer(p).and_then(Value::as_array) {
            return a.clone();
        }
    }
    vec![]
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.split_whitespace().next()?.parse().ok(),
        _ => None,
    }
}

/// Cuotas como "5 GB" o en bytes según la versión.
fn quota_gb(v: Option<&Value>) -> Option<f64> {
    let n = num(v)?;
    if n > 1024.0 * 1024.0 {
        Some((n / 1024f64.powi(3) * 10.0).round() / 10.0)
    } else {
        Some(n)
    }
}

fn s(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

pub enum Provider {
    AaPanel(AaPanel),
}

impl Provider {
    async fn list_domains(&self) -> Result<Vec<MailDomain>, String> {
        let Provider::AaPanel(p) = self;
        let data = p
            .plugin("get_domains", &[("p", "1".into()), ("size", "1000".into())])
            .await?;
        Ok(list_from(&data)
            .iter()
            .map(|d| MailDomain {
                domain: s(d, &["domain"]).unwrap_or_default(),
                mailboxes: num(d.get("mailbox_count").or(d.get("mailboxes"))).map(|n| n as i64),
                quota_gb: quota_gb(d.get("quota")),
                created_at: s(d, &["created"]),
            })
            .collect())
    }

    async fn add_domain(&self, domain: &str, hostname: &str) -> Result<(), String> {
        let Provider::AaPanel(p) = self;
        let a_record = if hostname.is_empty() {
            format!("mail.{domain}")
        } else {
            hostname.to_string()
        };
        p.plugin(
            "add_domain_new",
            &[
                ("domain", domain.into()),
                ("a_record", a_record),
                ("mailboxes", "50".into()),
                ("mailbox_quota", "5 GB".into()),
                ("quota", "50 GB".into()),
            ],
        )
        .await
        .map(|_| ())
    }

    async fn delete_domain(&self, domain: &str) -> Result<(), String> {
        let Provider::AaPanel(p) = self;
        p.plugin("delete_domain", &[("domain", domain.into())])
            .await
            .map(|_| ())
    }

    async fn list_mailboxes(&self, domain: &str) -> Result<Vec<Mailbox>, String> {
        let Provider::AaPanel(p) = self;
        let data = p
            .plugin(
                "get_mailboxs",
                &[
                    ("domain", domain.into()),
                    ("p", "1".into()),
                    ("size", "1000".into()),
                ],
            )
            .await?;
        Ok(list_from(&data)
            .iter()
            .map(|m| Mailbox {
                email: s(m, &["username", "email"]).unwrap_or_default(),
                full_name: s(m, &["full_name", "name"]).unwrap_or_default(),
                quota_gb: quota_gb(m.get("quota")),
                active: s(m, &["active"]).as_deref() != Some("0"),
                created_at: s(m, &["created"]),
            })
            .collect())
    }

    async fn add_mailbox(
        &self,
        email: &str,
        full_name: &str,
        password: &str,
        quota_gb: f64,
    ) -> Result<(), String> {
        let Provider::AaPanel(p) = self;
        p.plugin(
            "add_mailbox",
            &[
                ("username", email.into()),
                ("full_name", full_name.into()),
                ("password", password.into()),
                ("quota", format!("{quota_gb} GB")),
                ("is_admin", "0".into()),
                ("active", "1".into()),
            ],
        )
        .await
        .map(|_| ())
    }

    /// update_mailbox espera el registro completo: se combinan valores actuales y cambios.
    async fn update_mailbox(&self, cur: &Mailbox, patch: &MailboxPatch) -> Result<(), String> {
        let Provider::AaPanel(p) = self;
        let mut params = vec![
            ("username", cur.email.clone()),
            (
                "full_name",
                patch.full_name.clone().unwrap_or(cur.full_name.clone()),
            ),
            (
                "quota",
                format!("{} GB", patch.quota_gb.or(cur.quota_gb).unwrap_or(5.0)),
            ),
            (
                "active",
                if patch.active.unwrap_or(cur.active) {
                    "1"
                } else {
                    "0"
                }
                .into(),
            ),
            ("is_admin", "0".into()),
        ];
        if let Some(pw) = &patch.password {
            params.push(("password", pw.clone()));
        }
        p.plugin("update_mailbox", &params).await.map(|_| ())
    }

    async fn delete_mailbox(&self, email: &str) -> Result<(), String> {
        let Provider::AaPanel(p) = self;
        p.plugin("delete_mailbox", &[("username", email.into())])
            .await
            .map(|_| ())
    }
}

// ------------------------------------------------------------------ servicio

pub struct MailServerService {
    pub cfg: MailServerConfig,
    provider: Option<Provider>,
}

impl MailServerService {
    pub fn new(cfg: MailServerConfig) -> Self {
        let provider = match cfg.provider.as_str() {
            "aapanel" if !cfg.aapanel_api_key.is_empty() => {
                Some(Provider::AaPanel(AaPanel::new(&cfg)))
            }
            "aapanel" => {
                tracing::warn!("MAILSERVER_PROVIDER=aapanel pero falta AAPANEL_API_KEY: integración desactivada");
                None
            }
            _ => None,
        };
        Self { cfg, provider }
    }

    pub fn enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// Datos de conexión IMAP/SMTP de los buzones del servidor integrado.
    pub fn connection(&self) -> Value {
        json!({
            "imapHost": self.cfg.hostname, "imapPort": self.cfg.imap_port, "imapSecure": self.cfg.imap_port == 993,
            "smtpHost": self.cfg.hostname, "smtpPort": self.cfg.smtp_port, "smtpSecure": self.cfg.smtp_port == 465,
        })
    }

    fn p(&self) -> ApiResult<&Provider> {
        self.provider.as_ref().ok_or_else(|| {
            ApiError::unavailable("El servidor de correo integrado no está configurado")
        })
    }

    pub async fn status(&self) -> Value {
        let base = json!({ "provider": self.cfg.provider, "enabled": self.enabled(), "hostname": self.cfg.hostname });
        let Some(p) = &self.provider else {
            return merge(base, json!({ "ok": false, "error": null }));
        };
        match p.list_domains().await {
            Ok(d) => merge(
                base,
                json!({ "ok": true, "error": null, "domains": d.len() }),
            ),
            Err(e) => merge(base, json!({ "ok": false, "error": e })),
        }
    }

    /// ¿El dominio está alojado en el servidor integrado? (caché de 5 min en Redis)
    pub async fn is_managed_domain(&self, st: &Shared, domain: &str) -> bool {
        let Some(p) = &self.provider else {
            return false;
        };
        if self.cfg.hostname.is_empty() {
            return false;
        }
        let mut r = st.redis.clone();
        let cached: Option<String> = r.get(DOMAINS_CACHE).await.ok().flatten();
        let list: Vec<String> = match cached.and_then(|c| serde_json::from_str(&c).ok()) {
            Some(l) => l,
            None => match p.list_domains().await {
                Ok(d) => {
                    let l: Vec<String> = d.into_iter().map(|d| d.domain.to_lowercase()).collect();
                    let _: Result<(), _> = r
                        .set_ex(
                            DOMAINS_CACHE,
                            serde_json::to_string(&l).unwrap_or_default(),
                            300,
                        )
                        .await;
                    l
                }
                Err(e) => {
                    tracing::warn!(err = %e, "no se pudo leer la lista de dominios");
                    return false;
                }
            },
        };
        list.contains(&domain.to_lowercase())
    }
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        a.extend(b.clone());
    }
    a
}

fn gw(e: String) -> ApiError {
    ApiError::bad_gateway(format!("Servidor de correo: {e}"))
}

// ------------------------------------------------------------------ DNS

/// Registros que necesita un dominio para recibir y enviar sin caer en spam.
pub async fn check_dns(cfg: &MailServerConfig, domain: &str) -> Value {
    let server = if cfg.hostname.is_empty() {
        format!("mail.{domain}")
    } else {
        cfg.hostname.clone()
    };
    let Ok(builder) = TokioResolver::builder_tokio() else {
        return json!({ "domain": domain, "serverHost": server, "serverIps": [], "checks": [] });
    };
    let r = builder.build().expect("resolver");
    let a = |name: String| {
        let r = &r;
        async move {
            r.ipv4_lookup(name.as_str())
                .await
                .map(|l| {
                    l.answers()
                        .iter()
                        .filter_map(|rec| match &rec.data {
                            RData::A(a) => Some(a.0.to_string()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }
    };
    let txt = |name: String| {
        let r = &r;
        async move {
            r.txt_lookup(name.as_str())
                .await
                .map(|l| {
                    l.answers()
                        .iter()
                        .filter_map(|rec| match &rec.data {
                            RData::TXT(t) => Some(
                                t.txt_data
                                    .iter()
                                    .map(|b| String::from_utf8_lossy(b).to_string())
                                    .collect::<String>(),
                            ),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }
    };
    let server_ips = a(server.clone()).await;
    let mut mx: Vec<(u16, String)> = r
        .mx_lookup(domain)
        .await
        .map(|l| {
            l.answers()
                .iter()
                .filter_map(|rec| match &rec.data {
                    RData::MX(m) => Some((
                        m.preference,
                        m.exchange.to_string().trim_end_matches('.').to_string(),
                    )),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    mx.sort();
    let mut mx_ips = vec![];
    for (_, host) in &mx {
        mx_ips.extend(a(host.clone()).await);
    }
    let spf: Vec<String> = txt(domain.to_string())
        .await
        .into_iter()
        .filter(|t| t.to_lowercase().starts_with("v=spf1"))
        .collect();
    let dkim = txt(format!("default._domainkey.{domain}")).await;
    let dmarc = txt(format!("_dmarc.{domain}")).await;
    let short = |v: &[String]| {
        v.iter()
            .map(|t| {
                if t.chars().count() > 60 {
                    format!("{}…", t.chars().take(60).collect::<String>())
                } else {
                    t.clone()
                }
            })
            .collect::<Vec<_>>()
    };
    json!({
        "domain": domain, "serverHost": server, "serverIps": server_ips,
        "checks": [
            { "record": "A (servidor de correo)", "ok": !server_ips.is_empty(), "found": server_ips, "expected": format!("{server} → IP pública del VPS") },
            { "record": "MX", "ok": !mx.is_empty() && mx_ips.iter().any(|i| server_ips.contains(i)),
              "found": mx.iter().map(|(p, h)| format!("{p} {h}")).collect::<Vec<_>>(), "expected": format!("10 {server}") },
            { "record": "SPF (TXT)", "ok": spf.len() == 1, "found": spf, "expected": "v=spf1 mx a ~all   (un único registro SPF)" },
            { "record": "DKIM (default._domainkey)", "ok": dkim.iter().any(|t| t.contains("v=DKIM1") || t.contains("p=")),
              "found": short(&dkim), "expected": "Copia la clave DKIM desde el panel del servidor de correo" },
            { "record": "DMARC (_dmarc)", "ok": dmarc.iter().any(|t| t.to_lowercase().starts_with("v=dmarc1")),
              "found": dmarc, "expected": format!("v=DMARC1; p=quarantine; rua=mailto:postmaster@{domain}") },
        ]
    })
}

// ------------------------------------------------------------------ aprovisionamiento

async fn verify_mailbox_login(st: &Shared, email: &str, password: &str) -> bool {
    let c = &st.mailserver.cfg;
    let p = ImapParams {
        host: &c.hostname,
        port: c.imap_port,
        secure: c.imap_port == 993,
        username: email,
        password,
        allow_self_signed: st.core.allow_self_signed,
    };
    match imapnet::connect(&p).await {
        Ok(mut s) => {
            let _ = s.logout().await;
            true
        }
        Err(_) => false,
    }
}

/// Crea (o actualiza la contraseña de) la cuenta del buzón para el usuario.
async fn link_mailbox(
    st: &Shared,
    user_id: Uuid,
    email: &str,
    password: &str,
    display_name: &str,
) -> ApiResult<Uuid> {
    let c = &st.mailserver.cfg;
    let enc = st.core.master_key.encrypt(password);
    let existing: Option<Uuid> =
        sqlx::query_scalar("select id from mail_accounts where user_id = $1 and email = $2")
            .bind(user_id)
            .bind(email)
            .fetch_optional(&st.db)
            .await?;
    let id = match existing {
        Some(id) => {
            sqlx::query("update mail_accounts set secret_enc = $2, status = 'pending', last_error = null, enabled = true where id = $1")
                .bind(id)
                .bind(&enc)
                .execute(&st.db)
                .await?;
            id
        }
        None => sqlx::query_scalar(
            "insert into mail_accounts (user_id, display_name, email, username, secret_enc, source,
                   imap_host, imap_port, imap_secure, smtp_host, smtp_port, smtp_secure)
                 values ($1,$2,$3,$3,$4,'mailserver',$5,$6,$7,$5,$8,$9) returning id",
        )
        .bind(user_id)
        .bind(if display_name.is_empty() {
            email
        } else {
            display_name
        })
        .bind(email)
        .bind(&enc)
        .bind(&c.hostname)
        .bind(c.imap_port as i32)
        .bind(c.imap_port == 993)
        .bind(c.smtp_port as i32)
        .bind(c.smtp_port == 465)
        .fetch_one(&st.db)
        .await?,
    };
    notify(st, WorkerControl::AccountChanged { account_id: id }).await;
    Ok(id)
}

async fn update_linked_password(st: &Shared, email: &str, password: &str) -> ApiResult<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "update mail_accounts set secret_enc = $2, status = 'pending', last_error = null
         where email = $1 and source = 'mailserver' returning id",
    )
    .bind(email)
    .bind(st.core.master_key.encrypt(password))
    .fetch_all(&st.db)
    .await?;
    for id in ids {
        notify(st, WorkerControl::AccountChanged { account_id: id }).await;
    }
    Ok(())
}

async fn set_linked_enabled(st: &Shared, email: &str, enabled: bool) -> ApiResult<()> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("update mail_accounts set enabled = $2 where email = $1 and source = 'mailserver' returning id")
            .bind(email)
            .bind(enabled)
            .fetch_all(&st.db)
            .await?;
    for id in ids {
        notify(st, WorkerControl::AccountChanged { account_id: id }).await;
    }
    Ok(())
}

/// Inicio de sesión con las credenciales de un buzón alojado: si el IMAP las
/// acepta, se crea el usuario (o se sincroniza su contraseña) y el buzón queda
/// conectado como cuenta. Nunca aplica a administradores.
pub async fn mailbox_login(
    st: &Shared,
    email: &str,
    password: &str,
    existing: Option<User>,
) -> ApiResult<Option<User>> {
    if !st.mailserver.enabled() {
        return Ok(None);
    }
    let domain = email.split('@').nth(1).unwrap_or_default();
    if !st.mailserver.is_managed_domain(st, domain).await {
        return Ok(None);
    }
    if let Some(u) = &existing {
        if u.role != "user" {
            return Ok(None);
        }
        let linked: Option<Uuid> = sqlx::query_scalar(
            "select id from mail_accounts where user_id = $1 and email = $2 and source = 'mailserver'",
        )
        .bind(u.id)
        .bind(email)
        .fetch_optional(&st.db)
        .await?;
        if linked.is_none() {
            return Ok(None);
        }
    }
    if !verify_mailbox_login(st, email, password).await {
        return Ok(None);
    }
    let hash = hash_password(password);
    let user: User = match existing {
        Some(u) => sqlx::query_as("update users set password_hash = $2 where id = $1 returning *")
            .bind(u.id)
            .bind(&hash)
            .fetch_one(&st.db)
            .await?,
        None => sqlx::query_as("insert into users (email, name, password_hash, role) values ($1, $2, $3, 'user') returning *")
            .bind(email)
            .bind(email.split('@').next().unwrap_or(email))
            .bind(&hash)
            .fetch_one(&st.db)
            .await?,
    };
    link_mailbox(st, user.id, email, password, &user.name).await?;
    Ok(Some(user))
}

fn generate_password() -> String {
    // Sin símbolos ambiguos; los servidores de correo los aceptan todos
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    let mut s: String = buf
        .iter()
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect();
    s.push_str("A9");
    s
}

// ------------------------------------------------------------------ rutas

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/admin/mailserver", get(status_h))
        .route(
            "/admin/mailserver/domains",
            get(domains_h).post(add_domain_h),
        )
        .route(
            "/admin/mailserver/domains/{domain}",
            delete(delete_domain_h),
        )
        .route("/admin/mailserver/domains/{domain}/dns", get(dns_h))
        .route(
            "/admin/mailserver/domains/{domain}/mailboxes",
            get(mailboxes_h),
        )
        .route("/admin/mailserver/users", get(users_h))
        .route("/admin/mailserver/mailboxes", post(create_mailbox_h))
        .route(
            "/admin/mailserver/mailboxes/{email}",
            patch(update_mailbox_h).delete(delete_mailbox_h),
        )
        .route("/mailserver/autoconfig", get(autoconfig_h))
}

fn domain_param(raw: &str) -> ApiResult<String> {
    let d = raw.trim().to_lowercase();
    if validate::is_domain(&d) {
        Ok(d)
    } else {
        Err(ApiError::bad_request("Datos inválidos — dominio no válido"))
    }
}

async fn status_h(State(st): State<Shared>, Admin(_): Admin) -> Json<Value> {
    Json(st.mailserver.status().await)
}

async fn domains_h(State(st): State<Shared>, Admin(_): Admin) -> ApiResult<Json<Vec<MailDomain>>> {
    Ok(Json(st.mailserver.p()?.list_domains().await.map_err(gw)?))
}

#[derive(Deserialize)]
struct DomainInput {
    domain: String,
}

async fn add_domain_h(
    State(st): State<Shared>,
    Admin(_): Admin,
    Body(i): Body<DomainInput>,
) -> ApiResult<Json<Value>> {
    let d = domain_param(&i.domain)?;
    st.mailserver
        .p()?
        .add_domain(&d, &st.mailserver.cfg.hostname)
        .await
        .map_err(gw)?;
    let mut r = st.redis.clone();
    let _: i64 = r.del(DOMAINS_CACHE).await?;
    Ok(Json(
        json!({ "domain": d, "dns": check_dns(&st.mailserver.cfg, &d).await }),
    ))
}

async fn delete_domain_h(
    State(st): State<Shared>,
    Admin(_): Admin,
    Path(d): Path<String>,
) -> ApiResult<StatusCode> {
    let d = domain_param(&d)?;
    st.mailserver.p()?.delete_domain(&d).await.map_err(gw)?;
    let mut r = st.redis.clone();
    let _: i64 = r.del(DOMAINS_CACHE).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn dns_h(
    State(st): State<Shared>,
    Admin(_): Admin,
    Path(d): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        check_dns(&st.mailserver.cfg, &domain_param(&d)?).await,
    ))
}

async fn mailboxes_h(
    State(st): State<Shared>,
    Admin(_): Admin,
    Path(d): Path<String>,
) -> ApiResult<Json<Vec<Mailbox>>> {
    Ok(Json(
        st.mailserver
            .p()?
            .list_mailboxes(&domain_param(&d)?)
            .await
            .map_err(gw)?,
    ))
}

async fn users_h(State(st): State<Shared>, Admin(_): Admin) -> ApiResult<Json<Value>> {
    let rows: Vec<(Uuid, String, String, String)> =
        sqlx::query_as("select id, name, email, role from users order by name")
            .fetch_all(&st.db)
            .await?;
    Ok(Json(rows.into_iter().map(|(id, name, email, role)| json!({ "id": id, "name": name, "email": email, "role": role })).collect()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMailbox {
    email: String,
    full_name: String,
    /// Si se omite, se genera una contraseña segura y se devuelve una sola vez
    password: Option<String>,
    quota_gb: Option<f64>,
    /// none | me | user | new_user
    link: Option<String>,
    user_id: Option<Uuid>,
}

async fn create_mailbox_h(
    State(st): State<Shared>,
    Admin(admin): Admin,
    Body(i): Body<CreateMailbox>,
) -> ApiResult<Json<Value>> {
    let email = validate::email("email", &i.email)?;
    let full_name = i.full_name.trim().to_string();
    validate::len("fullName", &full_name, 1, 100)?;
    if let Some(p) = &i.password {
        validate::len("password", p, 10, 200)?;
    }
    let quota = i.quota_gb.unwrap_or(5.0);
    if !(0.1..=1024.0).contains(&quota) {
        return Err(ApiError::bad_request(
            "Datos inválidos — quotaGb: entre 0.1 y 1024",
        ));
    }
    let link = i.link.as_deref().unwrap_or("new_user");
    if !["none", "me", "user", "new_user"].contains(&link) {
        return Err(ApiError::bad_request(
            "Datos inválidos — link: valor no válido",
        ));
    }
    let domain = email.split('@').nth(1).unwrap_or_default().to_string();
    if !st.mailserver.is_managed_domain(&st, &domain).await {
        return Err(ApiError::bad_request(format!(
            "El dominio {domain} no está alojado en este servidor"
        )));
    }
    if link == "user" && i.user_id.is_none() {
        return Err(ApiError::bad_request(
            "Falta el usuario al que asignar el buzón",
        ));
    }
    if link == "new_user" {
        let exists: Option<Uuid> = sqlx::query_scalar("select id from users where email = $1")
            .bind(&email)
            .fetch_optional(&st.db)
            .await?;
        if exists.is_some() {
            return Err(ApiError::conflict(
                "Ya existe un usuario de RV Mail con ese correo; usa \"asignar a usuario\"",
            ));
        }
    }
    let generated = i.password.is_none();
    let password = i.password.unwrap_or_else(generate_password);
    st.mailserver
        .p()?
        .add_mailbox(&email, &full_name, &password, quota)
        .await
        .map_err(gw)?;

    let user_id = match link {
        "me" => Some(admin.id),
        "user" => i.user_id,
        "new_user" => Some(
            sqlx::query_scalar("insert into users (email, name, password_hash, role) values ($1,$2,$3,'user') returning id")
                .bind(&email)
                .bind(&full_name)
                .bind(hash_password(&password))
                .fetch_one(&st.db)
                .await?,
        ),
        _ => None,
    };
    if let Some(uid) = user_id {
        link_mailbox(&st, uid, &email, &password, &full_name).await?;
    }
    let mut out = json!({ "email": email, "linkedUserId": user_id });
    if generated {
        out["password"] = json!(password);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMailbox {
    password: Option<String>,
    full_name: Option<String>,
    quota_gb: Option<f64>,
    active: Option<bool>,
}

async fn update_mailbox_h(
    State(st): State<Shared>,
    Admin(_): Admin,
    Path(raw): Path<String>,
    Body(i): Body<UpdateMailbox>,
) -> ApiResult<Json<Value>> {
    let email = validate::email("email", &raw)?;
    if let Some(p) = &i.password {
        validate::len("password", p, 10, 200)?;
    }
    let p = st.mailserver.p()?;
    let domain = email.split('@').nth(1).unwrap_or_default();
    let cur = p
        .list_mailboxes(domain)
        .await
        .map_err(gw)?
        .into_iter()
        .find(|m| m.email.eq_ignore_ascii_case(&email))
        .ok_or_else(|| ApiError::bad_gateway("Servidor de correo: el buzón no existe"))?;
    let patch = MailboxPatch {
        password: i.password.clone(),
        full_name: i.full_name,
        quota_gb: i.quota_gb,
        active: i.active,
    };
    p.update_mailbox(&cur, &patch).await.map_err(gw)?;
    if let Some(pw) = &i.password {
        update_linked_password(&st, &email, pw).await?;
    }
    if let Some(a) = i.active {
        set_linked_enabled(&st, &email, a).await?;
    }
    Ok(Json(json!({ "email": email })))
}

async fn delete_mailbox_h(
    State(st): State<Shared>,
    Admin(_): Admin,
    Path(raw): Path<String>,
) -> ApiResult<StatusCode> {
    let email = validate::email("email", &raw)?;
    st.mailserver
        .p()?
        .delete_mailbox(&email)
        .await
        .map_err(gw)?;
    set_linked_enabled(&st, &email, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AutoconfigQuery {
    email: Option<String>,
}

/// Autoconfiguración de cuentas de dominios alojados (para el formulario de alta).
async fn autoconfig_h(
    State(st): State<Shared>,
    Session(_): Session,
    Query(q): Query<AutoconfigQuery>,
) -> ApiResult<Json<Value>> {
    let email = validate::email("email", q.email.as_deref().unwrap_or_default())?;
    let domain = email.split('@').nth(1).unwrap_or_default();
    if st.mailserver.is_managed_domain(&st, domain).await {
        Ok(Json(merge(
            json!({ "managed": true }),
            st.mailserver.connection(),
        )))
    } else {
        Ok(Json(json!({ "managed": false })))
    }
}
