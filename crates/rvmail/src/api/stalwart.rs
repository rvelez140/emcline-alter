//! Proveedor **Stalwart** (servidor de correo en Rust con JMAP, IMAP, SMTP,
//! CalDAV y CardDAV). Desde la 0.16 se administra por JMAP con la capacidad
//! `urn:stalwart:jmap` (objetos `x:Domain`, `x:Account`), verificado contra
//! Stalwart 0.16.26.

use super::mailserver::{MailDomain, Mailbox, MailboxPatch};
use crate::jmap::{Auth, Client, CORE};
use rvmail_core::config::MailServerConfig;
use serde_json::{json, Value};

const STALWART: &str = "urn:stalwart:jmap";

pub struct Stalwart {
    url: String,
    user: String,
    password: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ZoneRecord {
    pub name: String,
    pub rtype: String,
    pub value: String,
}

fn err(e: impl std::fmt::Display) -> String {
    tracing::warn!(err = %e, "Stalwart");
    let s = e.to_string();
    if s.contains("forbidden") {
        "Stalwart rechazó la operación (¿cuenta sin permisos de administrador?)".into()
    } else if s.contains("rechazó usuario") {
        "Usuario o contraseña de administrador de Stalwart incorrectos".into()
    } else if s.contains("conectar") {
        "No se pudo conectar con Stalwart (revisa STALWART_URL)".into()
    } else {
        format!("Stalwart: {s}")
    }
}

impl Stalwart {
    pub fn new(c: &MailServerConfig) -> Self {
        Self {
            url: c.stalwart_url.clone(),
            user: c.stalwart_user.clone(),
            password: c.stalwart_password.clone(),
            http: crate::net::tls::http_client(false),
        }
    }

    async fn client(&self) -> Result<Client, String> {
        Client::connect(
            &self.http,
            &self.url,
            Auth::Basic {
                user: self.user.clone(),
                pass: self.password.clone(),
            },
        )
        .await
        .map_err(err)
    }

    async fn call(&self, calls: Vec<(&str, Value)>) -> Result<Vec<Value>, String> {
        self.client()
            .await?
            .call(&[CORE, STALWART], calls)
            .await
            .map_err(err)
    }

    /// Resultado de un `x:…/set`: error si algo no se creó/actualizó/borró.
    fn check_set(r: &Value) -> Result<(), String> {
        for k in ["notCreated", "notUpdated", "notDestroyed"] {
            if let Some(m) = r
                .get(k)
                .and_then(Value::as_object)
                .filter(|m| !m.is_empty())
            {
                let e = m.values().next().cloned().unwrap_or_default();
                let d = e
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("operación rechazada");
                return Err(format!("Stalwart: {d}"));
            }
        }
        Ok(())
    }

    async fn domains(&self) -> Result<Vec<Value>, String> {
        let r = self
            .call(vec![
                ("x:Domain/query", json!({ "filter": {} })),
                ("x:Domain/get", json!({ "#ids": { "resultOf": "0", "name": "x:Domain/query", "path": "/ids" }, "properties": ["name", "createdAt"] })),
            ])
            .await?;
        Ok(r.get(1)
            .and_then(|g| g.get("list"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn domain_id(&self, domain: &str) -> Result<String, String> {
        self.domains()
            .await?
            .iter()
            .find(|d| {
                d.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.eq_ignore_ascii_case(domain))
            })
            .and_then(|d| d.get("id").and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| format!("El dominio {domain} no existe en Stalwart"))
    }

    async fn accounts(&self, domain_id: &str) -> Result<Vec<Value>, String> {
        let r = self
            .call(vec![
                ("x:Account/query", json!({ "filter": { "domainId": domain_id } })),
                ("x:Account/get", json!({ "#ids": { "resultOf": "0", "name": "x:Account/query", "path": "/ids" } })),
            ])
            .await?;
        Ok(r.get(1)
            .and_then(|g| g.get("list"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn account_id(&self, email: &str) -> Result<String, String> {
        let domain = email.split('@').nth(1).unwrap_or_default();
        let did = self.domain_id(domain).await?;
        self.accounts(&did)
            .await?
            .iter()
            .find(|a| {
                a.get("emailAddress")
                    .and_then(Value::as_str)
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
            })
            .and_then(|a| a.get("id").and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| "el buzón no existe".to_string())
    }

    pub async fn list_domains(&self) -> Result<Vec<MailDomain>, String> {
        let mut out = vec![];
        for d in self.domains().await? {
            let id = d.get("id").and_then(Value::as_str).unwrap_or_default();
            let count = self.accounts(id).await.map(|a| a.len() as i64).ok();
            out.push(MailDomain {
                domain: d
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mailboxes: count,
                quota_gb: None,
                created_at: d
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        Ok(out)
    }

    pub async fn add_domain(&self, domain: &str, _hostname: &str) -> Result<(), String> {
        // DKIM automático (Ed25519 + RSA); DNS y certificados los gestiona el administrador
        let r = self
            .call(vec![(
                "x:Domain/set",
                json!({ "create": { "d": {
                    "name": domain, "aliases": {},
                    "certificateManagement": { "@type": "Manual" },
                    "dkimManagement": { "@type": "Automatic" },
                    "dnsManagement": { "@type": "Manual" },
                    "subAddressing": { "@type": "Enabled" },
                } } }),
            )])
            .await?;
        Self::check_set(&r[0])
    }

    pub async fn delete_domain(&self, domain: &str) -> Result<(), String> {
        let id = self.domain_id(domain).await?;
        for _ in 0..2 {
            let r = self
                .call(vec![("x:Domain/set", json!({ "destroy": [id] }))])
                .await?;
            let Some(e) = r[0].pointer(&format!("/notDestroyed/{id}")) else {
                return Ok(());
            };
            if e.get("type").and_then(Value::as_str) != Some("objectIsLinked") {
                return Self::check_set(&r[0]);
            }
            // Objetos enlazados: las firmas DKIM se borran con el dominio; los buzones no
            let linked = e
                .get("linkedObjects")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if linked
                .iter()
                .any(|o| o.get("object").and_then(Value::as_str) != Some("DkimSignature"))
            {
                return Err("el dominio todavía tiene buzones o alias; elimínalos primero".into());
            }
            let dkim: Vec<&str> = linked
                .iter()
                .filter_map(|o| o.get("id").and_then(Value::as_str))
                .collect();
            let r = self
                .call(vec![("x:DkimSignature/set", json!({ "destroy": dkim }))])
                .await?;
            Self::check_set(&r[0])?;
        }
        Err("Stalwart no permitió eliminar el dominio".into())
    }

    pub async fn list_mailboxes(&self, domain: &str) -> Result<Vec<Mailbox>, String> {
        let did = self.domain_id(domain).await?;
        Ok(self
            .accounts(&did)
            .await?
            .iter()
            .filter(|a| a.get("@type").and_then(Value::as_str) == Some("User"))
            // La cuenta de administración de Stalwart no se lista (borrarla rompería la integración)
            .filter(|a| a.pointer("/roles/@type").and_then(Value::as_str) != Some("Admin"))
            .map(|a| Mailbox {
                email: a
                    .get("emailAddress")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                full_name: a
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                quota_gb: a
                    .pointer("/quotas/maxDiskQuota")
                    .and_then(Value::as_f64)
                    .map(|b| (b / 1024f64.powi(3) * 10.0).round() / 10.0),
                active: true,
                created_at: a
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
            .collect())
    }

    pub async fn add_mailbox(
        &self,
        email: &str,
        full_name: &str,
        password: &str,
        quota_gb: f64,
    ) -> Result<(), String> {
        let (local, domain) = email.split_once('@').ok_or("correo no válido")?;
        let did = self.domain_id(domain).await?;
        let r = self
            .call(vec![(
                "x:Account/set",
                json!({ "create": { "u": {
                    "@type": "User", "name": local, "domainId": did, "description": full_name,
                    "credentials": { "0": { "@type": "Password", "secret": password } },
                    "quotas": { "maxDiskQuota": (quota_gb * 1024f64.powi(3)) as u64 },
                } } }),
            )])
            .await?;
        Self::check_set(&r[0])
    }

    pub async fn update_mailbox(&self, cur: &Mailbox, patch: &MailboxPatch) -> Result<(), String> {
        if patch.active == Some(false) {
            return Err(
                "Stalwart no permite desactivar buzones: cambia la contraseña o elimínalo".into(),
            );
        }
        let id = self.account_id(&cur.email).await?;
        let mut upd = serde_json::Map::new();
        if let Some(n) = &patch.full_name {
            upd.insert("description".into(), json!(n));
        }
        if let Some(q) = patch.quota_gb {
            upd.insert(
                "quotas".into(),
                json!({ "maxDiskQuota": (q * 1024f64.powi(3)) as u64 }),
            );
        }
        if let Some(pw) = &patch.password {
            upd.insert(
                "credentials".into(),
                json!({ "0": { "@type": "Password", "secret": pw } }),
            );
        }
        if upd.is_empty() {
            return Ok(());
        }
        let r = self
            .call(vec![("x:Account/set", json!({ "update": { id: upd } }))])
            .await?;
        Self::check_set(&r[0])
    }

    pub async fn delete_mailbox(&self, email: &str) -> Result<(), String> {
        let id = self.account_id(email).await?;
        let r = self
            .call(vec![("x:Account/set", json!({ "destroy": [id] }))])
            .await?;
        Self::check_set(&r[0])
    }

    /// Zona DNS que genera Stalwart para el dominio (MX, SPF, DKIM, DMARC, SRV…).
    pub async fn expected_dns(&self, domain: &str) -> Result<Vec<ZoneRecord>, String> {
        let id = self.domain_id(domain).await?;
        let r = self
            .call(vec![(
                "x:Domain/get",
                json!({ "ids": [id], "properties": ["dnsZoneFile"] }),
            )])
            .await?;
        let zone = r[0]
            .pointer("/list/0/dnsZoneFile")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(parse_zone(zone))
    }
}

/// Lee un archivo de zona sencillo (una línea por registro; TXT largos entre paréntesis).
pub fn parse_zone(zone: &str) -> Vec<ZoneRecord> {
    let mut out = vec![];
    let mut pending = String::new();
    for line in zone.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        pending.push_str(line);
        pending.push(' ');
        if pending.matches('(').count() > pending.matches(')').count() {
            continue; // registro multilínea
        }
        let rec = std::mem::take(&mut pending);
        let mut it = rec.split_whitespace();
        let (Some(name), Some(_class), Some(rtype)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let value = if rtype == "TXT" {
            // Concatena las cadenas entre comillas conservando sus espacios
            rec.split('"').skip(1).step_by(2).collect::<String>()
        } else {
            it.filter(|t| *t != "(" && *t != ")")
                .collect::<Vec<_>>()
                .join(" ")
        };
        out.push(ZoneRecord {
            name: name.trim_end_matches('.').to_string(),
            rtype: rtype.to_string(),
            value: value.trim_end_matches('.').to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stalwart_zone() {
        let zone = r#"v1-ed25519-20261015._domainkey.rvtest.com. IN TXT "v=DKIM1; k=ed25519; h=sha256; p=6mxN"
v1-rsa-20261015._domainkey.rvtest.com. IN TXT (
    "v=DKIM1; k=rsa; h=sha256; p=MIIB"
    "IDAQAB"
)
rvtest.com. IN TXT "v=spf1 mx -all"
rvtest.com. IN MX 10 mail.rvtest.com.
_dmarc.rvtest.com. IN TXT "v=DMARC1; p=reject; rua=mailto:postmaster@rvtest.com"
"#;
        let r = parse_zone(zone);
        assert_eq!(r.len(), 5);
        assert_eq!(r[1].value, "v=DKIM1; k=rsa; h=sha256; p=MIIBIDAQAB");
        assert_eq!(
            r[3],
            ZoneRecord {
                name: "rvtest.com".into(),
                rtype: "MX".into(),
                value: "10 mail.rvtest.com".into()
            }
        );
    }
}
