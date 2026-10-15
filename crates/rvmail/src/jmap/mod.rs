//! Cliente JMAP (RFC 8620 núcleo, RFC 8621 correo): el protocolo moderno de
//! correo sobre HTTP/JSON con notificaciones push (EventSource). Lo usan el
//! worker (sincronización) y la API (leer, marcar, mover, enviar).

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;

pub const CORE: &str = "urn:ietf:params:jmap:core";
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";

#[derive(Clone)]
pub enum Auth {
    Basic { user: String, pass: String },
    Bearer(String),
}

impl Auth {
    fn header(&self) -> String {
        match self {
            Auth::Basic { user, pass } => format!("Basic {}", B64.encode(format!("{user}:{pass}"))),
            Auth::Bearer(t) => format!("Bearer {t}"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JmapError {
    #[error("El servidor rechazó usuario o contraseña")]
    Auth,
    #[error("{0}")]
    Other(String),
}

/// Sesión JMAP ya resuelta (`/.well-known/jmap`).
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    auth: Auth,
    pub api_url: String,
    pub event_source_url: String,
    pub account_id: String,
    pub can_submit: bool,
}

/// Reescribe una URL de la sesión al origen con el que nos conectamos: los
/// servidores anuncian su nombre público y RV Mail puede estar hablando con
/// ellos por la red interna (p. ej. http://stalwart:8080).
fn rebase(url: &str, base: &reqwest::Url) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_scheme(base.scheme());
            let _ = u.set_host(base.host_str());
            let _ = u.set_port(base.port());
            u.to_string()
        }
        // Relativa: se resuelve contra el origen
        Err(_) => base
            .join(url)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| url.to_string()),
    }
}

impl Client {
    /// Descubre la sesión. `session_url` puede ser la URL de la sesión o solo
    /// el origen (se añade `/.well-known/jmap`).
    pub async fn connect(
        http: &reqwest::Client,
        session_url: &str,
        auth: Auth,
    ) -> Result<Self, JmapError> {
        let mut url = session_url.trim().trim_end_matches('/').to_string();
        if !url.contains("/.well-known/jmap") && !url.contains("/jmap") {
            url.push_str("/.well-known/jmap");
        }
        let base = reqwest::Url::parse(&url)
            .map_err(|_| JmapError::Other("URL de JMAP no válida".into()))?;
        let res = http
            .get(url.as_str())
            .header("Authorization", auth.header())
            .send()
            .await
            .map_err(|e| JmapError::Other(net_error(&e)))?;
        if res.status() == reqwest::StatusCode::UNAUTHORIZED
            || res.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(JmapError::Auth);
        }
        if !res.status().is_success() {
            return Err(JmapError::Other(format!(
                "El servidor JMAP respondió HTTP {}",
                res.status()
            )));
        }
        let s: Value = res
            .json()
            .await
            .map_err(|_| JmapError::Other("Respuesta JMAP no válida".into()))?;
        let account_id = s
            .pointer(&format!("/primaryAccounts/{}", MAIL.replace('/', "~1")))
            .and_then(Value::as_str)
            .ok_or_else(|| JmapError::Other("La cuenta no tiene correo JMAP".into()))?
            .to_string();
        let get = |k: &str| {
            s.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Ok(Self {
            http: http.clone(),
            auth,
            api_url: rebase(&get("apiUrl"), &base),
            event_source_url: rebase(&get("eventSourceUrl"), &base),
            can_submit: s
                .pointer(&format!("/capabilities/{}", SUBMISSION.replace('/', "~1")))
                .is_some(),
            account_id,
        })
    }

    /// Ejecuta llamadas `[nombre, argumentos]` y devuelve las respuestas por id ("0", "1"…).
    /// Un error de método se convierte en error de la llamada completa.
    pub async fn call(&self, using: &[&str], calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
        let method_calls: Vec<Value> = calls
            .into_iter()
            .enumerate()
            .map(|(i, (name, mut args))| {
                if let Some(o) = args.as_object_mut() {
                    o.entry("accountId")
                        .or_insert_with(|| json!(self.account_id));
                }
                json!([name, args, i.to_string()])
            })
            .collect();
        let body = json!({ "using": using, "methodCalls": method_calls });
        let res = self
            .http
            .post(&self.api_url)
            .header("Authorization", self.auth.header())
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!(net_error(&e)))?;
        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            bail!(JmapError::Auth);
        }
        let status = res.status();
        let v: Value = res
            .json()
            .await
            .with_context(|| format!("respuesta JMAP no válida (HTTP {status})"))?;
        let responses = v
            .get("methodResponses")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("respuesta JMAP sin methodResponses"))?;
        let mut out = Vec::with_capacity(responses.len());
        for r in responses {
            let name = r.get(0).and_then(Value::as_str).unwrap_or_default();
            let args = r.get(1).cloned().unwrap_or(Value::Null);
            if name == "error" {
                let t = args.get("type").and_then(Value::as_str).unwrap_or("error");
                let d = args
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                bail!("error JMAP {t}: {d}");
            }
            out.push(args);
        }
        Ok(out)
    }

    pub async fn mailboxes(&self) -> Result<(Vec<Value>, String)> {
        let r = self
            .call(
                &[CORE, MAIL],
                vec![("Mailbox/get", json!({ "properties": ["name", "role", "parentId", "totalEmails", "unreadEmails", "sortOrder"] }))],
            )
            .await?;
        let list = r[0]
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let state = r[0]
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((list, state))
    }

    /// Ids de los correos de un buzón recibidos desde `after` (más recientes primero).
    pub async fn query_ids(&self, mailbox: &str, after: &str) -> Result<Vec<String>> {
        let mut ids = vec![];
        let mut position = 0;
        loop {
            let r = self
                .call(
                    &[CORE, MAIL],
                    vec![(
                        "Email/query",
                        json!({
                            "filter": { "inMailbox": mailbox, "after": after },
                            "sort": [{ "property": "receivedAt", "isAscending": false }],
                            "position": position,
                            "limit": 500,
                        }),
                    )],
                )
                .await?;
            let page: Vec<String> = r[0]
                .get("ids")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let n = page.len();
            ids.extend(page);
            if n < 500 {
                return Ok(ids);
            }
            position += 500;
        }
    }

    pub async fn get_emails(
        &self,
        ids: &[String],
        properties: &[&str],
    ) -> Result<(Vec<Value>, String)> {
        let mut all = vec![];
        let mut state = String::new();
        for chunk in ids.chunks(200) {
            let r = self
                .call(
                    &[CORE, MAIL],
                    vec![(
                        "Email/get",
                        json!({ "ids": chunk, "properties": properties }),
                    )],
                )
                .await?;
            all.extend(
                r[0].get("list")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            state = r[0]
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
        Ok((all, state))
    }

    /// Cuerpo completo: HTML, texto y adjuntos.
    pub async fn get_body(&self, id: &str) -> Result<Option<Value>> {
        let r = self
            .call(
                &[CORE, MAIL],
                vec![(
                    "Email/get",
                    json!({
                        "ids": [id],
                        "properties": ["htmlBody", "textBody", "attachments", "bodyValues"],
                        "fetchHTMLBodyValues": true,
                        "fetchTextBodyValues": true,
                        "maxBodyValueBytes": 2_000_000,
                    }),
                )],
            )
            .await?;
        Ok(r[0]
            .get("list")
            .and_then(Value::as_array)
            .and_then(|l| l.first().cloned()))
    }

    pub async fn set_email(&self, update: Value) -> Result<Value> {
        let r = self
            .call(&[CORE, MAIL], vec![("Email/set", update)])
            .await?;
        let res = r.into_iter().next().unwrap_or(Value::Null);
        for k in ["notCreated", "notUpdated", "notDestroyed"] {
            if let Some(m) = res
                .get(k)
                .and_then(Value::as_object)
                .filter(|m| !m.is_empty())
            {
                let e = m.values().next().cloned().unwrap_or_default();
                bail!(
                    "{} ({})",
                    e.get("type").and_then(Value::as_str).unwrap_or("error"),
                    e.get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                );
            }
        }
        Ok(res)
    }

    /// Crea el correo en `mailbox` y lo envía (EmailSubmission). Devuelve el id del correo.
    pub async fn submit(&self, email: Value, sent_mailbox: &str, from: &str) -> Result<String> {
        if !self.can_submit {
            bail!("el servidor JMAP no permite enviar desde esta cuenta (sin urn:ietf:params:jmap:submission)");
        }
        let ident = self
            .call(&[CORE, MAIL, SUBMISSION], vec![("Identity/get", json!({}))])
            .await?;
        let identities = ident[0]
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let identity = identities
            .iter()
            .find(|i| {
                i.get("email")
                    .and_then(Value::as_str)
                    .is_some_and(|e| e.eq_ignore_ascii_case(from))
            })
            .or(identities.first())
            .and_then(|i| i.get("id").and_then(Value::as_str))
            .ok_or_else(|| anyhow!("La cuenta no tiene identidad de envío"))?
            .to_string();
        let mut email = email;
        email["mailboxIds"] = json!({ sent_mailbox: true });
        email["keywords"] = json!({ "$seen": true });
        let r = self
            .call(
                &[CORE, MAIL, SUBMISSION],
                vec![
                    ("Email/set", json!({ "create": { "e1": email } })),
                    (
                        "EmailSubmission/set",
                        json!({ "create": { "s1": { "emailId": "#e1", "identityId": identity } } }),
                    ),
                ],
            )
            .await?;
        if let Some(e) = r[0].pointer("/notCreated/e1") {
            bail!(
                "no se pudo crear el correo: {}",
                e.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }
        if let Some(e) = r.get(1).and_then(|s| s.pointer("/notCreated/s1")) {
            bail!(
                "el servidor rechazó el envío: {}",
                e.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }
        r[0].pointer("/created/e1/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("respuesta de envío incompleta"))
    }

    /// Escucha cambios (EventSource). Devuelve cuando llega un StateChange o se cierra.
    pub async fn wait_change(&self, timeout: std::time::Duration) -> Result<bool> {
        let url = self
            .event_source_url
            .replace("{types}", "Email,Mailbox")
            .replace("{closeafter}", "state")
            .replace("{ping}", "60");
        let res = self
            .http
            .get(&url)
            .header("Authorization", self.auth.header())
            .header("Accept", "text/event-stream")
            .timeout(timeout + std::time::Duration::from_secs(30))
            .send()
            .await?;
        if !res.status().is_success() {
            bail!("EventSource HTTP {}", res.status());
        }
        let mut stream = res.bytes_stream();
        let mut buf = String::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let chunk = tokio::select! {
                c = stream.next() => c,
                _ = tokio::time::sleep_until(deadline) => return Ok(false),
            };
            let Some(chunk) = chunk else { return Ok(false) };
            buf.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(i) = buf.find("\n\n") {
                let event: String = buf.drain(..i + 2).collect();
                if event.lines().any(|l| {
                    l.trim() == "event: state"
                        || l.starts_with("data:") && l.contains("StateChange")
                }) {
                    return Ok(true);
                }
            }
        }
    }
}

fn net_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "Tiempo de espera agotado al conectar".into()
    } else if e.is_connect() {
        "No se pudo conectar con el servidor JMAP".into()
    } else {
        "Error de comunicación con el servidor JMAP".into()
    }
}

/// Rol JMAP → special-use de IMAP (mismo modelo de carpetas en la BD).
pub fn role_to_special(role: Option<&str>) -> Option<String> {
    Some(
        match role? {
            "inbox" => "\\Inbox",
            "sent" => "\\Sent",
            "drafts" => "\\Drafts",
            "trash" => "\\Trash",
            "junk" => "\\Junk",
            "archive" => "\\Archive",
            _ => return None,
        }
        .to_string(),
    )
}

/// Ruta legible de un buzón ("Clientes/2026") a partir de su jerarquía.
pub fn mailbox_paths(list: &[Value]) -> HashMap<String, String> {
    let by_id: HashMap<&str, &Value> = list
        .iter()
        .filter_map(|m| Some((m.get("id")?.as_str()?, m)))
        .collect();
    let mut out = HashMap::new();
    for (id, m) in &by_id {
        let mut parts = vec![m
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()];
        let mut parent = m.get("parentId").and_then(Value::as_str);
        let mut guard = 0;
        while let (Some(p), true) = (parent, guard < 20) {
            let Some(pm) = by_id.get(p) else { break };
            parts.push(
                pm.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
            parent = pm.get("parentId").and_then(Value::as_str);
            guard += 1;
        }
        parts.reverse();
        out.insert(id.to_string(), parts.join("/"));
    }
    out
}

/// UID numérico estable para un id JMAP (la tabla exige un entero por carpeta).
pub fn stable_uid(id: &str) -> i64 {
    // FNV-1a de 64 bits, recortado a positivo
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h & 0x7fff_ffff_ffff_ffff) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebases_public_urls() {
        let base = reqwest::Url::parse("http://stalwart:8080/.well-known/jmap").unwrap();
        assert_eq!(
            rebase("https://mail.rvtest.com/jmap/", &base),
            "http://stalwart:8080/jmap/"
        );
        assert_eq!(rebase("/jmap/", &base), "http://stalwart:8080/jmap/");
    }

    #[test]
    fn paths_and_uids() {
        let list = vec![
            json!({"id":"a","name":"Clientes"}),
            json!({"id":"b","name":"2026","parentId":"a"}),
        ];
        let p = mailbox_paths(&list);
        assert_eq!(p["b"], "Clientes/2026");
        assert_eq!(stable_uid("Mabc"), stable_uid("Mabc"));
        assert!(stable_uid("x") >= 0);
        assert_eq!(role_to_special(Some("sent")).as_deref(), Some("\\Sent"));
    }
}
