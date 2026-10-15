//! Autodescubrimiento de la configuración de una cuenta a partir del correo:
//!  1. buzón del servidor integrado (aaPanel / Stalwart),
//!  2. JMAP (`/.well-known/jmap`, RFC 8620 §2.2),
//!  3. autoconfig del dominio y base de datos de Thunderbird (ISPDB),
//!  4. registros MX de proveedores conocidos (Google, Microsoft, Yahoo…).

use super::auth::Session;
use super::error::ApiResult;
use super::state::Shared;
use super::validate;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use hickory_resolver::proto::rr::RData;
use hickory_resolver::TokioResolver;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub fn routes() -> Router<Shared> {
    Router::new().route("/accounts/discover", get(discover))
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    pub secure: bool,
}

#[derive(Deserialize)]
struct Q {
    email: Option<String>,
}

async fn discover(
    State(st): State<Shared>,
    Session(_): Session,
    Query(q): Query<Q>,
) -> ApiResult<Json<Value>> {
    let email = validate::email("email", q.email.as_deref().unwrap_or_default())?;
    let domain = email.split('@').nth(1).unwrap_or_default().to_string();

    if st.mailserver.is_managed_domain(&st, &domain).await {
        let mut v = json!({ "source": "mailserver", "managed": true });
        if let Some(url) = st.mailserver.jmap_url() {
            v["jmapUrl"] = json!(url);
        }
        if let (Some(o), Some(m)) = (st.mailserver.connection().as_object(), v.as_object_mut()) {
            m.extend(o.clone());
        }
        return Ok(Json(v));
    }

    let http = crate::net::tls::http_client(false);
    let (jmap, ispdb) = tokio::join!(
        find_jmap(&http, &domain),
        find_ispdb(&http, &domain, &email)
    );
    if let Some((imap, smtp)) = ispdb.or(by_mx(&http, &domain, &email).await) {
        return Ok(Json(json!({
            "source": "autoconfig", "managed": false, "jmapUrl": jmap,
            "imapHost": imap.host, "imapPort": imap.port, "imapSecure": imap.secure,
            "smtpHost": smtp.host, "smtpPort": smtp.port, "smtpSecure": smtp.secure,
        })));
    }
    if let Some(url) = jmap {
        return Ok(Json(
            json!({ "source": "jmap", "managed": false, "jmapUrl": url }),
        ));
    }
    Ok(Json(json!({ "source": null, "managed": false })))
}

/// ¿Hay un servidor JMAP? Sin credenciales responde 401 o una sesión JSON.
async fn find_jmap(http: &reqwest::Client, domain: &str) -> Option<String> {
    for host in [
        domain.to_string(),
        format!("mail.{domain}"),
        format!("jmap.{domain}"),
    ] {
        let url = format!("https://{host}/.well-known/jmap");
        let Ok(Ok(res)) = tokio::time::timeout(Duration::from_secs(5), http.get(&url).send()).await
        else {
            continue;
        };
        let ok = res.status() == reqwest::StatusCode::UNAUTHORIZED
            || (res.status().is_success()
                && res
                    .json::<Value>()
                    .await
                    .ok()
                    .is_some_and(|v| v.get("capabilities").is_some()));
        if ok {
            return Some(format!("https://{host}"));
        }
    }
    None
}

/// Autoconfig (formato de Thunderbird): primero el propio dominio, luego la ISPDB.
async fn find_ispdb(
    http: &reqwest::Client,
    domain: &str,
    email: &str,
) -> Option<(ServerSettings, ServerSettings)> {
    let urls = [
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={email}"),
        format!(
            "https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml?emailaddress={email}"
        ),
        format!("https://autoconfig.thunderbird.net/v1.1/{domain}"),
    ];
    for url in urls {
        let Ok(Ok(res)) = tokio::time::timeout(Duration::from_secs(6), http.get(&url).send()).await
        else {
            continue;
        };
        if !res.status().is_success() {
            continue;
        }
        let Ok(xml) = res.text().await else { continue };
        if let Some(found) = parse_autoconfig(&xml, domain, email) {
            return Some(found);
        }
    }
    None
}

/// Proveedores grandes por su MX; si no se reconoce, se pregunta a la ISPDB por el dominio del MX.
async fn by_mx(
    http: &reqwest::Client,
    domain: &str,
    email: &str,
) -> Option<(ServerSettings, ServerSettings)> {
    let r = TokioResolver::builder_tokio().ok()?.build().ok()?;
    let lookup = tokio::time::timeout(Duration::from_secs(5), r.mx_lookup(domain))
        .await
        .ok()?
        .ok()?;
    let mx: Vec<String> = lookup
        .answers()
        .iter()
        .filter_map(|rec| match &rec.data {
            RData::MX(m) => Some(m.exchange.to_string().trim_end_matches('.').to_lowercase()),
            _ => None,
        })
        .collect();
    let s = |h: &str, p: u16, sec: bool| ServerSettings {
        host: h.into(),
        port: p,
        secure: sec,
    };
    for host in &mx {
        if host.ends_with("google.com") || host.ends_with("googlemail.com") {
            return Some((
                s("imap.gmail.com", 993, true),
                s("smtp.gmail.com", 465, true),
            ));
        }
        if host.ends_with("outlook.com") || host.ends_with("protection.outlook.com") {
            return Some((
                s("outlook.office365.com", 993, true),
                s("smtp.office365.com", 587, false),
            ));
        }
        if host.contains("yahoodns.net") {
            return Some((
                s("imap.mail.yahoo.com", 993, true),
                s("smtp.mail.yahoo.com", 465, true),
            ));
        }
    }
    // MX en un proveedor de hosting conocido por la ISPDB (p. ej. mx01.ionos.es → ionos.es)
    let base = mx.first()?.rsplitn(3, '.').collect::<Vec<_>>();
    if base.len() >= 2 {
        let mx_domain = format!("{}.{}", base[1], base[0]);
        return find_ispdb(http, &mx_domain, email).await;
    }
    None
}

fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let start = block.find(&format!("<{name}>"))? + name.len() + 2;
    let end = block[start..].find(&format!("</{name}>"))? + start;
    Some(block[start..end].trim())
}

fn server_block<'a>(xml: &'a str, open: &str) -> Option<&'a str> {
    let start = xml.find(open)?;
    let rest = &xml[start..];
    let close = if open.starts_with("<incoming") {
        "</incomingServer>"
    } else {
        "</outgoingServer>"
    };
    let end = rest.find(close)?;
    Some(&rest[..end])
}

/// Extrae IMAP y SMTP del XML de autoconfig de Thunderbird.
pub fn parse_autoconfig(
    xml: &str,
    domain: &str,
    email: &str,
) -> Option<(ServerSettings, ServerSettings)> {
    let fill = |h: &str| {
        h.replace("%EMAILDOMAIN%", domain)
            .replace("%EMAILADDRESS%", email)
            .replace(
                "%EMAILLOCALPART%",
                email.split('@').next().unwrap_or_default(),
            )
    };
    let parse = |block: &str, default_secure_port: u16| -> Option<ServerSettings> {
        let host = fill(tag(block, "hostname")?);
        let port: u16 = tag(block, "port")?.parse().ok()?;
        let socket = tag(block, "socketType").unwrap_or("SSL");
        Some(ServerSettings {
            host,
            port,
            secure: socket.eq_ignore_ascii_case("SSL") || port == default_secure_port,
        })
    };
    let imap = server_block(xml, "<incomingServer type=\"imap\"").and_then(|b| parse(b, 993))?;
    let smtp = server_block(xml, "<outgoingServer type=\"smtp\"").and_then(|b| parse(b, 465))?;
    Some((imap, smtp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thunderbird_autoconfig() {
        let xml = r#"<clientConfig version="1.1"><emailProvider id="ejemplo.com">
          <incomingServer type="pop3"><hostname>pop.ejemplo.com</hostname><port>995</port><socketType>SSL</socketType></incomingServer>
          <incomingServer type="imap"><hostname>imap.%EMAILDOMAIN%</hostname><port>993</port><socketType>SSL</socketType></incomingServer>
          <outgoingServer type="smtp"><hostname>smtp.ejemplo.com</hostname><port>587</port><socketType>STARTTLS</socketType></outgoingServer>
        </emailProvider></clientConfig>"#;
        let (imap, smtp) = parse_autoconfig(xml, "cliente.do", "ana@cliente.do").unwrap();
        assert_eq!(
            imap,
            ServerSettings {
                host: "imap.cliente.do".into(),
                port: 993,
                secure: true
            }
        );
        assert_eq!(
            smtp,
            ServerSettings {
                host: "smtp.ejemplo.com".into(),
                port: 587,
                secure: false
            }
        );
    }
}
