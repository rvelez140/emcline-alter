//! Correo: lógica compartida por la API REST (navegador/apps) y el servidor MCP
//! (agentes de IA). Toda consulta se limita a las cuentas del usuario y, si la
//! petición llega con token, a las cuentas y ámbitos que el token permite.

use super::auth::{Authed, SessionUser};
use super::clients::{self, conn_for, humanize_smtp, imap_fail};
use super::error::{ApiError, ApiResult};
use super::state::Shared;
use super::validate::{self, Body};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::AsyncTransport;
use mail_parser::{MessageParser, MimeHeaders, PartType};
use redis::AsyncCommands;
use rvmail_core::events::{WorkerControl, WORKER_CONTROL_CHANNEL};
use rvmail_core::models::{Address, Folder, MailAccount};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::types::Json as SqlJson;
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

pub const SPECIALS: [&str; 6] = ["inbox", "sent", "drafts", "trash", "junk", "archive"];
const BODY_CACHE_TTL: u64 = 3600;

fn special_use(s: &str) -> Option<&'static str> {
    Some(match s {
        "inbox" => "\\Inbox",
        "sent" => "\\Sent",
        "drafts" => "\\Drafts",
        "trash" => "\\Trash",
        "junk" => "\\Junk",
        "archive" => "\\Archive",
        _ => return None,
    })
}

/// Restringe una consulta a las cuentas visibles (usuario y, si hay token, sus cuentas).
fn push_scope(q: &mut QueryBuilder<Postgres>, user: &SessionUser) {
    q.push(" a.user_id = ").push_bind(user.id);
    if let Some(ids) = user.token.as_ref().and_then(|t| t.account_ids.clone()) {
        q.push(" and a.id = any(").push_bind(ids).push(")");
    }
}

// ------------------------------------------------------------------ tipos

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: Uuid,
    pub display_name: String,
    pub email: String,
    pub status: String,
    pub enabled: bool,
    pub source: String,
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub id: Uuid,
    pub account_id: Uuid,
    pub path: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub special_use: Option<String>,
    pub unread_count: i32,
    pub total_count: i32,
}

#[derive(Serialize, FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub id: Uuid,
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub to: SqlJson<Vec<Address>>,
    pub date: DateTime<Utc>,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub account_color: String,
    pub account_email: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub account_id: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    /// Carpeta especial para la vista unificada (inbox por defecto)
    pub special: Option<String>,
    pub q: Option<String>,
    pub unread: Option<String>,
    /// "<ISO date>|<id>" del último elemento de la página anterior
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub items: Vec<MessageSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub filename: Option<String>,
    pub content_type: String,
    pub size: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MessageBody {
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(FromRow)]
struct Owned {
    id: Uuid,
    account_id: Uuid,
    folder_id: Uuid,
    uid: i64,
    message_id: Option<String>,
    subject: String,
    from_name: Option<String>,
    from_address: Option<String>,
    to: SqlJson<Vec<Address>>,
    cc: SqlJson<Vec<Address>>,
    date: DateTime<Utc>,
    snippet: String,
    seen: bool,
    flagged: bool,
    has_attachments: bool,
    folder_path: String,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ComposeInput {
    pub account_id: Uuid,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub html: String,
    pub text: Option<String>,
    pub in_reply_to_message_id: Option<Uuid>,
}

impl ComposeInput {
    fn validate(mut self, require_to: bool) -> ApiResult<Self> {
        self.to = validate::emails("to", &self.to)?;
        self.cc = validate::emails("cc", &self.cc)?;
        self.bcc = validate::emails("bcc", &self.bcc)?;
        if require_to && self.to.is_empty() {
            return Err(ApiError::bad_request(
                "Datos inválidos — to: al menos un destinatario",
            ));
        }
        validate::len("subject", &self.subject, 0, 998)?;
        validate::len("html", &self.html, 0, 5_000_000)?;
        Ok(self)
    }
}

// ------------------------------------------------------------------ servicio

pub async fn list_accounts(st: &Shared, user: &SessionUser) -> ApiResult<Vec<AccountSummary>> {
    let mut q = QueryBuilder::new(
        "select a.id, a.display_name, a.email, a.status, a.enabled, a.source from mail_accounts a where",
    );
    push_scope(&mut q, user);
    q.push(" order by a.sort_order, a.created_at");
    Ok(q.build_query_as().fetch_all(&st.db).await?)
}

pub async fn list_folders(
    st: &Shared,
    user: &SessionUser,
    account: Option<Uuid>,
) -> ApiResult<Vec<FolderDto>> {
    let mut q = QueryBuilder::new(
        "select f.id, f.account_id, f.path, f.name, f.delimiter, f.special_use, f.unread_count, f.total_count
         from folders f join mail_accounts a on a.id = f.account_id where",
    );
    push_scope(&mut q, user);
    if let Some(id) = account {
        q.push(" and f.account_id = ").push_bind(id);
    }
    q.push(" order by f.account_id, f.path");
    Ok(q.build_query_as().fetch_all(&st.db).await?)
}

/// Sin folderId → vista unificada de la carpeta especial (inbox por defecto)
/// de todas las cuentas o de accountId.
pub async fn list_messages(
    st: &Shared,
    user: &SessionUser,
    p: &ListQuery,
) -> ApiResult<MessagePage> {
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    let special = p.special.as_deref().unwrap_or("inbox");
    let special = special_use(special)
        .ok_or_else(|| ApiError::bad_request("Datos inválidos — special: valor no válido"))?;
    let mut q = QueryBuilder::new(
        "select m.id, m.account_id, m.folder_id, m.subject, m.from_name, m.from_address, m.\"to\", m.date, m.snippet,
                m.seen, m.flagged, m.has_attachments, a.color as account_color, a.email as account_email
         from messages m
         join folders f on f.id = m.folder_id
         join mail_accounts a on a.id = m.account_id
         where",
    );
    push_scope(&mut q, user);
    q.push(" and a.enabled = true");
    if let Some(id) = p.account_id {
        q.push(" and m.account_id = ").push_bind(id);
    }
    match p.folder_id {
        Some(id) => q.push(" and m.folder_id = ").push_bind(id),
        None => q.push(" and f.special_use = ").push_bind(special),
    };
    if p.unread.as_deref() == Some("true") {
        q.push(" and m.seen = false");
    }
    if let Some(text) = p.q.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        validate::len("q", text, 0, 200)?;
        q.push(" and m.search @@ websearch_to_tsquery('spanish', ")
            .push_bind(text.to_string())
            .push(")");
    }
    if let Some((d, id)) = p.cursor.as_deref().and_then(|c| c.split_once('|')) {
        if let (Ok(date), Ok(id)) = (DateTime::parse_from_rfc3339(d), Uuid::parse_str(id)) {
            let date = date.with_timezone(&Utc);
            q.push(" and (m.date < ")
                .push_bind(date)
                .push(" or (m.date = ")
                .push_bind(date);
            q.push(" and m.id < ").push_bind(id).push("))");
        }
    }
    q.push(" order by m.date desc, m.id desc limit ")
        .push_bind(limit + 1);
    let mut items: Vec<MessageSummary> = q.build_query_as().fetch_all(&st.db).await?;
    let has_more = items.len() as i64 > limit;
    items.truncate(limit as usize);
    let next_cursor = if has_more {
        items.last().map(|m| {
            format!(
                "{}|{}",
                m.date.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                m.id
            )
        })
    } else {
        None
    };
    Ok(MessagePage { items, next_cursor })
}

async fn get_owned(st: &Shared, user: &SessionUser, id: Uuid) -> ApiResult<(Owned, MailAccount)> {
    let mut q = QueryBuilder::new(
        "select m.id, m.account_id, m.folder_id, m.uid, m.message_id, m.subject, m.from_name, m.from_address,
                m.\"to\", m.cc, m.date, m.snippet, m.seen, m.flagged, m.has_attachments, f.path as folder_path
         from messages m join folders f on f.id = m.folder_id join mail_accounts a on a.id = m.account_id
         where m.id = ",
    );
    q.push_bind(id).push(" and");
    push_scope(&mut q, user);
    let msg: Owned = q
        .build_query_as()
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Mensaje no encontrado"))?;
    let acc: MailAccount = sqlx::query_as("select * from mail_accounts where id = $1")
        .bind(msg.account_id)
        .fetch_one(&st.db)
        .await?;
    Ok((msg, acc))
}

pub async fn get_account(st: &Shared, user: &SessionUser, id: Uuid) -> ApiResult<MailAccount> {
    let mut q = QueryBuilder::new("select a.* from mail_accounts a where a.id = ");
    q.push_bind(id).push(" and");
    push_scope(&mut q, user);
    q.build_query_as()
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Cuenta no encontrada"))
}

pub fn parse_body(raw: &[u8]) -> MessageBody {
    let Some(m) = MessageParser::default().parse(raw) else {
        return MessageBody {
            html: None,
            text: None,
            attachments: vec![],
        };
    };
    // Solo HTML real: mail-parser convierte texto en HTML si no hay parte HTML
    let html = m
        .html_body
        .first()
        .and_then(|&i| m.parts.get(i as usize))
        .and_then(|p| match &p.body {
            PartType::Html(h) => Some(h.to_string()),
            _ => None,
        });
    let text = m.body_text(0).map(|t| t.into_owned());
    let attachments = m
        .attachments
        .iter()
        .filter_map(|&i| m.parts.get(i as usize))
        .map(|p| AttachmentInfo {
            filename: p.attachment_name().map(str::to_string),
            content_type: p
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: p.len(),
        })
        .collect();
    MessageBody {
        html,
        text,
        attachments,
    }
}

/// Cuerpo completo: se descarga por IMAP bajo demanda y se cachea en Redis.
pub async fn get_message(
    st: &Shared,
    user: &SessionUser,
    id: Uuid,
    mark_seen: bool,
) -> ApiResult<Value> {
    let (msg, acc) = get_owned(st, user, id).await?;
    let key = format!("rvmail:body:{id}");
    let mut r = st.redis.clone();
    let cached: Option<String> = r.get(&key).await?;
    let body: MessageBody = match cached.and_then(|c| serde_json::from_str(&c).ok()) {
        Some(b) => b,
        None => {
            let conn = conn_for(st, &acc).await?;
            let mut s = clients::open(st, &conn).await?;
            let res = async {
                s.examine(&msg.folder_path).await?;
                let f: Vec<_> = s
                    .uid_fetch(msg.uid.to_string(), "(UID BODY.PEEK[])")
                    .await?
                    .try_collect()
                    .await?;
                anyhow::Ok(f.first().and_then(|f| f.body().map(<[u8]>::to_vec)))
            }
            .await;
            clients::close(s).await;
            let raw = res
                .map_err(imap_fail)?
                .ok_or_else(|| ApiError::not_found("El mensaje ya no existe en el servidor"))?;
            let b = parse_body(&raw);
            let _: () = r
                .set_ex(&key, serde_json::to_string(&b)?, BODY_CACHE_TTL)
                .await?;
            b
        }
    };
    let mut seen = msg.seen;
    if !seen && mark_seen {
        set_flags(st, user, id, Some(true), None).await?;
        seen = true;
    }
    Ok(json!({
        "id": msg.id, "accountId": msg.account_id, "folderId": msg.folder_id, "messageId": msg.message_id,
        "subject": msg.subject, "fromName": msg.from_name, "fromAddress": msg.from_address,
        "to": msg.to.0, "cc": msg.cc.0, "date": msg.date, "snippet": msg.snippet,
        "seen": seen, "flagged": msg.flagged, "hasAttachments": msg.has_attachments,
        "accountColor": acc.color, "accountEmail": acc.email,
        "html": body.html, "text": body.text, "attachments": body.attachments,
    }))
}

pub async fn set_flags(
    st: &Shared,
    user: &SessionUser,
    id: Uuid,
    seen: Option<bool>,
    flagged: Option<bool>,
) -> ApiResult<Value> {
    user.require("mail:modify")?;
    let (msg, acc) = get_owned(st, user, id).await?;
    let conn = conn_for(st, &acc).await?;
    let mut s = clients::open(st, &conn).await?;
    let uid = msg.uid.to_string();
    let res = async {
        s.select(&msg.folder_path).await?;
        for (on, flag) in [(seen, "\\Seen"), (flagged, "\\Flagged")] {
            if let Some(on) = on {
                let op = if on { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
                let _: Vec<_> = s
                    .uid_store(&uid, format!("{op} ({flag})"))
                    .await?
                    .try_collect()
                    .await?;
            }
        }
        anyhow::Ok(())
    }
    .await;
    clients::close(s).await;
    res.map_err(imap_fail)?;

    let row: (bool, bool) = sqlx::query_as(
        "update messages set seen = coalesce($2, seen), flagged = coalesce($3, flagged) where id = $1 returning seen, flagged",
    )
    .bind(id)
    .bind(seen)
    .bind(flagged)
    .fetch_one(&st.db)
    .await?;
    if let Some(s) = seen.filter(|s| *s != msg.seen) {
        sqlx::query(
            "update folders set unread_count = greatest(unread_count + $2, 0) where id = $1",
        )
        .bind(msg.folder_id)
        .bind(if s { -1 } else { 1 })
        .execute(&st.db)
        .await?;
    }
    Ok(json!({ "id": id, "seen": row.0, "flagged": row.1 }))
}

/// Mueve a la papelera (o borra definitivamente si ya está en la papelera).
pub async fn remove(st: &Shared, user: &SessionUser, id: Uuid) -> ApiResult<()> {
    user.require("mail:modify")?;
    let (msg, acc) = get_owned(st, user, id).await?;
    let trash: Option<Folder> =
        sqlx::query_as("select * from folders where account_id = $1 and special_use = '\\Trash'")
            .bind(acc.id)
            .fetch_optional(&st.db)
            .await?;
    let conn = conn_for(st, &acc).await?;
    let mut s = clients::open(st, &conn).await?;
    let uid = msg.uid.to_string();
    let res = async {
        s.select(&msg.folder_path).await?;
        match trash.as_ref().filter(|t| t.id != msg.folder_id) {
            Some(t) => {
                // MOVE (RFC 6851); si no existe, COPY + \Deleted + EXPUNGE
                if s.uid_mv(&uid, &t.path).await.is_err() {
                    s.uid_copy(&uid, &t.path).await?;
                    delete_uid(&mut s, &uid).await?;
                }
            }
            None => delete_uid(&mut s, &uid).await?,
        }
        anyhow::Ok(())
    }
    .await;
    clients::close(s).await;
    res.map_err(imap_fail)?;
    sqlx::query("delete from messages where id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    request_sync(st, acc.id).await;
    Ok(())
}

async fn delete_uid(s: &mut crate::net::imap::ImapSession, uid: &str) -> anyhow::Result<()> {
    let _: Vec<_> = s
        .uid_store(uid, "+FLAGS.SILENT (\\Deleted)")
        .await?
        .try_collect()
        .await?;
    // UID EXPUNGE (UIDPLUS) borra solo este mensaje; si no existe, EXPUNGE general
    let done = match s.uid_expunge(uid).await {
        Ok(stream) => stream.try_collect::<Vec<_>>().await.is_ok(),
        Err(_) => false,
    };
    if !done {
        let _: Vec<_> = s.expunge().await?.try_collect().await?;
    }
    Ok(())
}

pub async fn request_sync(st: &Shared, account_id: Uuid) {
    let mut r = st.redis.clone();
    let msg =
        serde_json::to_string(&WorkerControl::AccountSync { account_id }).expect("serializable");
    let _: Result<i64, _> = r.publish(WORKER_CONTROL_CHANNEL, msg).await;
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let lower = html.to_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if !in_tag && lower[i..].starts_with("<br") {
            out.push('\n');
        }
        let c = html[i..].chars().next().unwrap_or(' ');
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
        i += c.len_utf8();
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn mailbox(name: Option<&str>, addr: &str) -> ApiResult<Mailbox> {
    let email = addr
        .parse()
        .map_err(|_| ApiError::bad_request(format!("Correo no válido: {addr}")))?;
    Ok(Mailbox::new(name.map(str::to_string), email))
}

/// Construye el mensaje MIME (texto + HTML) con firma y cabeceras de respuesta.
async fn build(
    st: &Shared,
    acc: &MailAccount,
    i: &ComposeInput,
) -> ApiResult<(lettre::Message, String)> {
    let mut reply: Option<String> = None;
    if let Some(orig) = i.in_reply_to_message_id {
        reply =
            sqlx::query_scalar("select message_id from messages where id = $1 and account_id = $2")
                .bind(orig)
                .bind(acc.id)
                .fetch_optional(&st.db)
                .await?
                .flatten();
    }
    let html = match &acc.signature_html {
        Some(sig) => format!("{}<br><br>{sig}", i.html),
        None => i.html.clone(),
    };
    let text = i.text.clone().unwrap_or_else(|| html_to_text(&html));
    let domain = acc.email.split('@').nth(1).unwrap_or("rvmail.local");
    let message_id = format!("<{}@{domain}>", Uuid::new_v4());

    let mut b = lettre::Message::builder()
        .from(mailbox(Some(&acc.display_name), &acc.email)?)
        .subject(i.subject.clone())
        .message_id(Some(message_id.clone()))
        .date_now();
    for t in &i.to {
        b = b.to(mailbox(None, t)?);
    }
    for c in &i.cc {
        b = b.cc(mailbox(None, c)?);
    }
    for c in &i.bcc {
        b = b.bcc(mailbox(None, c)?);
    }
    if let Some(r) = reply {
        b = b.in_reply_to(r.clone()).references(r);
    }
    let body = MultiPart::alternative()
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(text),
        )
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(html),
        );
    // Borradores sin destinatario: lettre exige un sobre; se usa el propio remitente
    let msg = if i.to.is_empty() && i.cc.is_empty() && i.bcc.is_empty() {
        b.to(mailbox(None, &acc.email)?).multipart(body)
    } else {
        b.multipart(body)
    }
    .map_err(|_| ApiError::bad_request("No se pudo construir el mensaje"))?;
    Ok((msg, message_id))
}

pub async fn send(st: &Shared, user: &SessionUser, i: ComposeInput) -> ApiResult<Value> {
    user.require("mail:send")?;
    let i = i.validate(true)?;
    let acc = get_account(st, user, i.account_id).await?;
    if let Some(t) = &user.token {
        enforce_agent_limit(st, t.id).await?;
    }
    let (msg, message_id) = build(st, &acc, &i).await?;
    let conn = conn_for(st, &acc).await?;
    let smtp = clients::smtp(st, &conn)?;
    smtp.send(msg.clone())
        .await
        .map_err(|e| ApiError::bad_gateway(format!("No se pudo enviar: {}", humanize_smtp(&e))))?;
    if let Some(t) = &user.token {
        tracing::info!(token = %t.name, user = %user.email, from = %acc.email, recipients = i.to.len() + i.cc.len() + i.bcc.len(), "agente envió correo");
    }

    // Copia en "Enviados" (muchos servidores no lo hacen solos)
    if st.cfg.append_sent {
        let sent: Option<String> = sqlx::query_scalar(
            "select path from folders where account_id = $1 and special_use = '\\Sent'",
        )
        .bind(acc.id)
        .fetch_optional(&st.db)
        .await?;
        if let Some(path) = sent {
            if let Ok(mut s) = clients::open(st, &conn).await {
                let _ = s
                    .append(&path, Some("(\\Seen)"), None, msg.formatted())
                    .await;
                clients::close(s).await;
            }
        }
    }
    request_sync(st, acc.id).await;
    Ok(json!({ "messageId": message_id }))
}

/// Guarda un borrador en "Borradores": la vía segura para agentes de IA.
pub async fn save_draft(st: &Shared, user: &SessionUser, i: ComposeInput) -> ApiResult<Value> {
    user.require("mail:draft")?;
    let i = i.validate(false)?;
    let acc = get_account(st, user, i.account_id).await?;
    let drafts: Option<String> = sqlx::query_scalar(
        "select path from folders where account_id = $1 and special_use = '\\Drafts'",
    )
    .bind(acc.id)
    .fetch_optional(&st.db)
    .await?;
    let exists = drafts.is_some();
    let path = drafts.unwrap_or_else(|| "Drafts".into());
    let (msg, _) = build(st, &acc, &i).await?;
    let conn = conn_for(st, &acc).await?;
    let mut s = clients::open(st, &conn).await?;
    let res = async {
        if !exists {
            let _ = s.create(&path).await; // si la cuenta no tiene carpeta de borradores
        }
        s.append(&path, Some("(\\Draft \\Seen)"), None, msg.formatted())
            .await?;
        anyhow::Ok(())
    }
    .await;
    clients::close(s).await;
    res.map_err(|e| {
        tracing::warn!(err = %format!("{e:#}"), "no se pudo guardar el borrador");
        ApiError::bad_gateway("No se pudo guardar el borrador")
    })?;
    request_sync(st, acc.id).await;
    Ok(json!({ "folder": path }))
}

/// Límite de envíos por hora para cada token: protege la reputación del dominio.
async fn enforce_agent_limit(st: &Shared, token: Uuid) -> ApiResult<()> {
    let key = format!("rvmail:rl:agent-send:{token}");
    let mut r = st.redis.clone();
    let n: i64 = r.incr(&key, 1).await?;
    if n == 1 {
        let _: bool = r.expire(&key, 3600).await?;
    }
    if n > st.cfg.agent_send_limit_per_hour {
        return Err(ApiError::too_many(format!(
            "Límite de {} envíos por hora alcanzado para este token",
            st.cfg.agent_send_limit_per_hour
        )));
    }
    Ok(())
}

// ------------------------------------------------------------------ rutas REST

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/folders", get(folders_h))
        .route("/messages", get(list_h))
        .route("/messages/{id}", get(get_h).patch(patch_h).delete(delete_h))
        .route("/send", post(send_h))
        .route("/drafts", post(draft_h))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FoldersQuery {
    account_id: Option<Uuid>,
}

async fn folders_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    Query(q): Query<FoldersQuery>,
) -> ApiResult<Json<Vec<FolderDto>>> {
    u.require("mail:read")?;
    Ok(Json(list_folders(&st, &u, q.account_id).await?))
}

async fn list_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    q: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<MessagePage>> {
    u.require("mail:read")?;
    let Query(q) =
        q.map_err(|_| ApiError::bad_request("Datos inválidos — parámetros de búsqueda"))?;
    Ok(Json(list_messages(&st, &u, &q).await?))
}

async fn get_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    u.require("mail:read")?;
    // Un agente con solo lectura no altera el estado "leído" del buzón
    let mark = u.has("mail:modify");
    Ok(Json(get_message(&st, &u, id, mark).await?))
}

#[derive(Deserialize)]
struct PatchInput {
    seen: Option<bool>,
    flagged: Option<bool>,
}

async fn patch_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    Path(id): Path<Uuid>,
    Body(p): Body<PatchInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(set_flags(&st, &u, id, p.seen, p.flagged).await?))
}

async fn delete_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    remove(&st, &u, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn send_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    Body(i): Body<ComposeInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(send(&st, &u, i).await?))
}

async fn draft_h(
    State(st): State<Shared>,
    Authed(u): Authed,
    Body(i): Body<ComposeInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    Ok((StatusCode::CREATED, Json(save_draft(&st, &u, i).await?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_only_when_real() {
        let text_only = b"Subject: x\r\nContent-Type: text/plain\r\n\r\nhola";
        let b = parse_body(text_only);
        assert!(b.html.is_none());
        assert_eq!(b.text.as_deref(), Some("hola"));
        let html = b"Subject: x\r\nContent-Type: text/html\r\n\r\n<p>hola</p>";
        assert!(parse_body(html).html.unwrap().contains("<p>hola</p>"));
    }

    #[test]
    fn html_to_text_keeps_breaks() {
        assert_eq!(html_to_text("a<br>b &amp; <b>c</b>"), "a\nb & c");
    }
}
