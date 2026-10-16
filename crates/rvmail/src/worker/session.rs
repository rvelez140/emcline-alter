//! Una sesión IMAP persistente por cuenta:
//!  1. sincroniza carpetas y encabezados recientes,
//!  2. se queda en IDLE sobre INBOX (notificación inmediata de correo nuevo),
//!  3. repasa periódicamente las demás carpetas, saltando las que no cambiaron
//!     (STATUS con UIDNEXT/MESSAGES/UNSEEN y, si hay CONDSTORE, HIGHESTMODSEQ).
//!
//! Respecto a la versión Node, además detecta borrados y cambios de marcas en
//! cada pasada (no solo con IDLE), lo que cubre la sincronización incremental.

use super::parse::{self, MessageRow};
use super::special_use;
use crate::net::imap::{self as imapnet, ImapError, ImapParams, ImapSession};
use anyhow::{Context, Result};
use async_imap::extensions::idle::IdleResponse;
use async_imap::imap_proto::SectionPath;
use async_imap::types::{Flag, NameAttribute};
use chrono::{Duration as ChronoDuration, Utc};
use futures::TryStreamExt;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use rvmail_core::config::{CoreConfig, WorkerConfig};
use rvmail_core::events::{events_channel, AccountStatus, RvEvent};
use rvmail_core::models::{Folder, MailAccount};
use sqlx::types::Json;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{sleep, sleep_until, Instant};
use tracing::{debug, info, warn};
use uuid::Uuid;

const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// RFC 2177: renovar IDLE antes de 29 minutos
const IDLE_RENEW: Duration = Duration::from_secs(25 * 60);
const FLAGS_CHUNK: usize = 500;
/// Partes de texto mayores se piden parcialmente (solo los primeros bytes)
const SNIPPET_FULL_LIMIT: u32 = 16_384;

/// Error con un mensaje ya apto para el usuario (español, sin detalles internos).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct UserFacing(String);

#[derive(Debug)]
pub enum Command {
    Sync,
    Stop,
}

#[derive(Clone)]
pub struct Ctx {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub core: CoreConfig,
    pub cfg: WorkerConfig,
    /// Cliente HTTP (refresco de tokens OAuth)
    pub http: reqwest::Client,
}

/// Estado de una carpeta según el último STATUS, para saltar las que no cambiaron.
#[derive(Clone, PartialEq, Eq, Debug)]
struct FolderState {
    uid_validity: Option<u32>,
    uid_next: Option<u32>,
    exists: u32,
    unseen: Option<u32>,
    modseq: Option<u64>,
}

#[path = "session_jmap.rs"]
mod jmap_impl;

enum Outcome {
    Stopped,
    AuthFailed,
}

pub struct AccountSession {
    account_id: Uuid,
    ctx: Ctx,
    rx: mpsc::Receiver<Command>,
}

impl AccountSession {
    pub fn new(account_id: Uuid, ctx: Ctx, rx: mpsc::Receiver<Command>) -> Self {
        Self {
            account_id,
            ctx,
            rx,
        }
    }

    /// Bucle de vida de la cuenta: conecta, sincroniza, IDLE; reconecta con backoff.
    pub async fn run(mut self) {
        let mut backoff = Duration::from_secs(5);
        loop {
            match self.connect_and_serve().await {
                Ok(Outcome::Stopped) => return,
                Ok(Outcome::AuthFailed) => {
                    // No reintentar: el usuario debe corregir la contraseña o reconectar la cuenta OAuth
                    let oauth: bool = sqlx::query_scalar(
                        "select auth_type = 'oauth2' from mail_accounts where id = $1",
                    )
                    .bind(self.account_id)
                    .fetch_optional(&self.ctx.db)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                    let msg = if oauth {
                        "El proveedor rechazó el acceso; vuelve a conectar la cuenta"
                    } else {
                        "El servidor rechazó usuario o contraseña"
                    };
                    self.set_status(AccountStatus::AuthError, Some(msg)).await;
                    self.wait_for_stop().await;
                    return;
                }
                Err(e) => {
                    // Los errores ya redactados para el usuario se muestran tal cual
                    let msg = match e.downcast_ref::<UserFacing>() {
                        Some(UserFacing(m)) => m.clone(),
                        None => imapnet::humanize(&e),
                    };
                    warn!(account = %self.account_id, err = %format!("{e:#}"), "sesión IMAP interrumpida");
                    self.set_status(AccountStatus::Error, Some(&msg)).await;
                }
            }
            info!(account = %self.account_id, wait_s = backoff.as_secs(), "reconectando");
            tokio::select! {
                _ = sleep(backoff) => {}
                cmd = self.rx.recv() => {
                    if matches!(cmd, None | Some(Command::Stop)) { return; }
                }
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn wait_for_stop(&mut self) {
        while let Some(cmd) = self.rx.recv().await {
            if matches!(cmd, Command::Stop) {
                return;
            }
        }
    }

    async fn load_account(&self) -> Result<Option<MailAccount>> {
        Ok(
            sqlx::query_as::<_, MailAccount>("select * from mail_accounts where id = $1")
                .bind(self.account_id)
                .fetch_optional(&self.ctx.db)
                .await?,
        )
    }

    async fn connect_and_serve(&mut self) -> Result<Outcome> {
        let Some(account) = self.load_account().await? else {
            return Ok(Outcome::Stopped);
        };
        if !account.enabled {
            return Ok(Outcome::Stopped);
        }
        self.set_status(AccountStatus::Syncing, None).await;
        if account.protocol == "jmap" {
            return self.serve_jmap(account).await;
        }
        let connected = if account.auth_type == "oauth2" {
            // Gmail / Outlook / Yahoo: access token fresco y XOAUTH2
            let token = match crate::oauth::access_token(
                &self.ctx.db,
                &self.ctx.redis,
                &self.ctx.core.master_key,
                &self.ctx.http,
                &account,
            )
            .await
            {
                Ok(t) => t,
                Err(crate::oauth::OAuthError::Revoked) => return Ok(Outcome::AuthFailed),
                Err(crate::oauth::OAuthError::Other(m)) => return Err(UserFacing(m).into()),
            };
            imapnet::connect_xoauth2(
                &account.imap_host,
                account.imap_port as u16,
                account.imap_secure,
                &account.username,
                &token,
                self.ctx.core.allow_self_signed,
            )
            .await
        } else {
            let password = self.ctx.core.master_key.decrypt(&account.secret_enc)?;
            imapnet::connect(&ImapParams {
                host: &account.imap_host,
                port: account.imap_port as u16,
                secure: account.imap_secure,
                username: &account.username,
                password: &password,
                allow_self_signed: self.ctx.core.allow_self_signed,
            })
            .await
        };
        let mut session = match connected {
            Ok(s) => s,
            Err(ImapError::Auth) => return Ok(Outcome::AuthFailed),
            Err(ImapError::Other(m)) => return Err(UserFacing(m).into()),
        };
        let caps = session.capabilities().await?;
        let condstore = caps.has_str("CONDSTORE") || caps.has_str("QRESYNC");
        let can_idle = caps.has_str("IDLE");

        let mut sync = Syncer {
            account: &account,
            ctx: &self.ctx,
            condstore,
            states: HashMap::new(),
            inbox: None,
        };
        sync.full_sync(&mut session).await?;
        self.set_status(AccountStatus::Idle, None).await;
        info!(account = %account.email, condstore, can_idle, "cuenta sincronizada");

        let mut next_full = Instant::now() + self.ctx.cfg.sync_interval;
        loop {
            let inbox = sync.inbox.clone();
            // Sin IDLE (o sin INBOX): solo sondeo periódico
            let wake = if let (true, Some(inbox)) = (can_idle, inbox) {
                session.select(&inbox.path).await?;
                let mut handle = session.idle();
                handle.init().await?;
                let wake = {
                    let (fut, stop) = handle.wait_with_timeout(IDLE_RENEW);
                    tokio::pin!(fut);
                    tokio::select! {
                        r = &mut fut => match r? {
                            IdleResponse::NewData(_) => Wake::ServerData,
                            _ => Wake::Renew,
                        },
                        _ = sleep_until(next_full) => { drop(stop); fut.await?; Wake::Periodic }
                        cmd = self.rx.recv() => { drop(stop); fut.await?; Wake::Cmd(cmd) }
                    }
                };
                session = handle.done().await?;
                wake
            } else {
                tokio::select! {
                    _ = sleep_until(next_full) => Wake::Periodic,
                    cmd = self.rx.recv() => Wake::Cmd(cmd),
                }
            };

            match wake {
                Wake::Renew => {}
                Wake::ServerData => {
                    if let Some(inbox) = sync.inbox.clone() {
                        sync.sync_folder(&mut session, &inbox, true).await?;
                    }
                }
                Wake::Periodic => {
                    sync.full_sync(&mut session).await?;
                    next_full = Instant::now() + self.ctx.cfg.sync_interval;
                }
                Wake::Cmd(Some(Command::Sync)) => sync.full_sync(&mut session).await?,
                Wake::Cmd(None | Some(Command::Stop)) => {
                    let _ = session.logout().await;
                    return Ok(Outcome::Stopped);
                }
            }
        }
    }

    async fn set_status(&self, status: AccountStatus, error: Option<&str>) {
        let r = sqlx::query(
            "update mail_accounts set status = $2, last_error = $3 where id = $1 returning user_id",
        )
        .bind(self.account_id)
        .bind(status.as_str())
        .bind(error)
        .fetch_optional(&self.ctx.db)
        .await;
        if let Ok(Some(row)) = r {
            use sqlx::Row;
            let user_id: Uuid = row.get(0);
            publish(
                &self.ctx.redis,
                user_id,
                &RvEvent::AccountStatus {
                    account_id: self.account_id,
                    status,
                    error: error.map(str::to_string),
                },
            )
            .await;
        }
    }
}

enum Wake {
    ServerData,
    Renew,
    Periodic,
    Cmd(Option<Command>),
}

pub async fn publish(redis: &ConnectionManager, user_id: Uuid, event: &RvEvent) {
    let mut c = redis.clone();
    let payload = serde_json::to_string(event).expect("evento serializable");
    if let Err(e) = c
        .publish::<_, _, i64>(events_channel(user_id), payload)
        .await
    {
        warn!(err = %e, "no se pudo publicar evento");
    }
}

struct Syncer<'a> {
    account: &'a MailAccount,
    ctx: &'a Ctx,
    condstore: bool,
    states: HashMap<String, FolderState>,
    inbox: Option<Folder>,
}

impl Syncer<'_> {
    async fn full_sync(&mut self, s: &mut ImapSession) -> Result<()> {
        let folders = self.sync_folder_list(s).await?;
        for f in &folders {
            self.sync_folder(s, f, false).await?;
        }
        sqlx::query("update mail_accounts set last_sync_at = now() where id = $1")
            .bind(self.account.id)
            .execute(&self.ctx.db)
            .await?;
        Ok(())
    }

    async fn sync_folder_list(&mut self, s: &mut ImapSession) -> Result<Vec<Folder>> {
        let names: Vec<_> = s.list(Some(""), Some("*")).await?.try_collect().await?;
        let mut result = Vec::new();
        for n in &names {
            let attrs = n.attributes();
            if attrs.iter().any(|a| matches!(a, NameAttribute::NoSelect))
                || attrs.iter().any(|a| matches!(a, NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\NonExistent")))
            {
                continue;
            }
            let announced = attrs.iter().find_map(|a| match a {
                NameAttribute::All => Some("\\All"),
                NameAttribute::Archive => Some("\\Archive"),
                NameAttribute::Drafts => Some("\\Drafts"),
                NameAttribute::Flagged => Some("\\Flagged"),
                NameAttribute::Junk => Some("\\Junk"),
                NameAttribute::Sent => Some("\\Sent"),
                NameAttribute::Trash => Some("\\Trash"),
                NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\Important") => {
                    Some("\\Important")
                }
                _ => None,
            });
            let path = n.name();
            let leaf = match n.delimiter() {
                Some(d) if !d.is_empty() => path.rsplit(d).next().unwrap_or(path),
                _ => path,
            };
            let special = special_use::resolve(path, leaf, announced);
            if special_use::skip(special.as_deref()) {
                continue;
            }
            let row = sqlx::query_as::<_, Folder>(
                "insert into folders (account_id, path, name, delimiter, special_use)
                 values ($1, $2, $3, $4, $5)
                 on conflict (account_id, path) do update
                   set name = excluded.name, delimiter = excluded.delimiter, special_use = excluded.special_use
                 returning *",
            )
            .bind(self.account.id)
            .bind(path)
            .bind(leaf)
            .bind(n.delimiter())
            .bind(&special)
            .fetch_one(&self.ctx.db)
            .await?;
            result.push(row);
        }
        // Carpetas que ya no existen en el servidor
        let paths: Vec<&str> = result.iter().map(|f| f.path.as_str()).collect();
        if !paths.is_empty() {
            sqlx::query("delete from folders where account_id = $1 and path <> all($2)")
                .bind(self.account.id)
                .bind(&paths)
                .execute(&self.ctx.db)
                .await?;
        }
        result.sort_by_key(|f| special_use::priority(f.special_use.as_deref()));
        self.inbox = result
            .iter()
            .find(|f| f.special_use.as_deref() == Some("\\Inbox"))
            .cloned();
        Ok(result)
    }

    /// Sincroniza una carpeta. `force` ignora la caché de STATUS (llegó un aviso por IDLE).
    async fn sync_folder(
        &mut self,
        s: &mut ImapSession,
        folder: &Folder,
        force: bool,
    ) -> Result<()> {
        // El estado en BD puede haber cambiado (last_uid, uid_validity): releer
        let folder = sqlx::query_as::<_, Folder>("select * from folders where id = $1")
            .bind(folder.id)
            .fetch_one(&self.ctx.db)
            .await?;

        let items = if self.condstore {
            "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN HIGHESTMODSEQ)"
        } else {
            "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN)"
        };
        let st = s
            .status(&folder.path, items)
            .await
            .with_context(|| format!("STATUS {}", folder.path))?;
        let state = FolderState {
            uid_validity: st.uid_validity,
            uid_next: st.uid_next,
            exists: st.exists,
            unseen: st.unseen,
            modseq: st.highest_modseq,
        };
        // Carpeta sin cambios desde la última pasada: no se abre. Sin CONDSTORE los
        // cambios de marcas no se ven en STATUS, así que INBOX se revisa siempre.
        let unchanged = self.states.get(&folder.path) == Some(&state);
        let is_inbox = folder.special_use.as_deref() == Some("\\Inbox");
        if unchanged && !force && (self.condstore || !is_inbox) {
            return Ok(());
        }

        let changed = self.reconcile(s, &folder, &state).await?;
        sqlx::query("update folders set total_count = $2, unread_count = $3 where id = $1")
            .bind(folder.id)
            .bind(state.exists as i32)
            .bind(state.unseen.unwrap_or(0) as i32)
            .execute(&self.ctx.db)
            .await?;
        self.states.insert(folder.path.clone(), state);

        if changed.inserted > 0 {
            publish(
                &self.ctx.redis,
                self.account.user_id,
                &RvEvent::MessageNew {
                    account_id: self.account.id,
                    folder_id: folder.id,
                    count: changed.inserted,
                },
            )
            .await;
        }
        if !changed.updated.is_empty() {
            publish(
                &self.ctx.redis,
                self.account.user_id,
                &RvEvent::MessageUpdated {
                    account_id: self.account.id,
                    message_ids: changed.updated,
                },
            )
            .await;
        }
        if changed.deleted > 0 || changed.inserted > 0 {
            publish(
                &self.ctx.redis,
                self.account.user_id,
                &RvEvent::FolderUpdated {
                    account_id: self.account.id,
                    folder_id: folder.id,
                },
            )
            .await;
        }
        Ok(())
    }

    /// Descarga mensajes nuevos, elimina los borrados y actualiza marcas dentro de la ventana de días.
    async fn reconcile(
        &mut self,
        s: &mut ImapSession,
        folder: &Folder,
        state: &FolderState,
    ) -> Result<Changes> {
        let mut changes = Changes::default();
        let db = &self.ctx.db;
        let mut last_uid = folder.last_uid;

        if let (Some(old), Some(new)) = (folder.uid_validity, state.uid_validity) {
            if old != i64::from(new) {
                info!(folder = %folder.path, "UIDVALIDITY cambió, resincronizando carpeta");
                sqlx::query("delete from messages where folder_id = $1")
                    .bind(folder.id)
                    .execute(db)
                    .await?;
                last_uid = 0;
            }
        }

        if state.exists == 0 {
            let r = sqlx::query("delete from messages where folder_id = $1")
                .bind(folder.id)
                .execute(db)
                .await?;
            changes.deleted = r.rows_affected() as usize;
            self.save_folder(folder, state, last_uid).await?;
            return Ok(changes);
        }

        s.examine(&folder.path).await?;
        let since = Utc::now() - ChronoDuration::days(self.ctx.cfg.sync_days);
        let present: HashSet<u32> = s
            .uid_search(format!("SINCE {}", since.format("%d-%b-%Y")))
            .await?;

        // Locales dentro de la ventana
        let local: Vec<(Uuid, i64, Json<Vec<String>>)> = sqlx::query_as(
            "select id, uid, flags from messages where folder_id = $1 and date >= $2",
        )
        .bind(folder.id)
        .bind(since - ChronoDuration::days(1))
        .fetch_all(db)
        .await?;
        let local_uids: HashSet<u32> = local.iter().map(|(_, u, _)| *u as u32).collect();

        // 1. Borrados (EXPUNGE / movidos a otra carpeta)
        let gone: Vec<Uuid> = local
            .iter()
            .filter(|(_, u, _)| !present.contains(&(*u as u32)))
            .map(|(id, _, _)| *id)
            .collect();
        if !gone.is_empty() {
            sqlx::query("delete from messages where id = any($1)")
                .bind(&gone)
                .execute(db)
                .await?;
            changes.deleted = gone.len();
        }

        // 2. Nuevos (más recientes primero)
        let mut new: Vec<u32> = present
            .iter()
            .copied()
            .filter(|u| !local_uids.contains(u))
            .collect();
        new.sort_unstable_by(|a, b| b.cmp(a));
        for batch in new.chunks(self.ctx.cfg.batch_size.max(1)) {
            let rows = self.fetch_rows(s, folder, batch).await?;
            changes.inserted += self.insert_rows(&rows).await?;
        }
        let max_uid = present.iter().copied().max().map(i64::from).unwrap_or(0);
        last_uid = last_uid.max(max_uid);

        // 3. Marcas de los que ya teníamos
        let known: Vec<&(Uuid, i64, Json<Vec<String>>)> = local
            .iter()
            .filter(|(_, u, _)| present.contains(&(*u as u32)))
            .collect();
        if !known.is_empty() {
            let by_uid: HashMap<u32, &(Uuid, i64, Json<Vec<String>>)> =
                known.iter().map(|r| (r.1 as u32, *r)).collect();
            let uids: Vec<u32> = by_uid.keys().copied().collect();
            for chunk in uids.chunks(FLAGS_CHUNK) {
                let set = uid_set(chunk);
                let fetched: Vec<_> = s
                    .uid_fetch(&set, "(UID FLAGS)")
                    .await?
                    .try_collect()
                    .await?;
                for f in &fetched {
                    let Some(uid) = f.uid else { continue };
                    let Some((id, _, old)) = by_uid.get(&uid).copied() else {
                        continue;
                    };
                    let flags = flag_strings(f.flags());
                    if sorted(&flags) != sorted(&old.0) {
                        sqlx::query(
                            "update messages set flags = $2, seen = $3, flagged = $4 where id = $1",
                        )
                        .bind(id)
                        .bind(Json(&flags))
                        .bind(flags.iter().any(|x| x == "\\Seen"))
                        .bind(flags.iter().any(|x| x == "\\Flagged"))
                        .execute(db)
                        .await?;
                        changes.updated.push(*id);
                    }
                }
            }
        }

        self.save_folder(folder, state, last_uid).await?;
        debug!(folder = %folder.path, ?changes, "carpeta sincronizada");
        Ok(changes)
    }

    async fn save_folder(&self, folder: &Folder, state: &FolderState, last_uid: i64) -> Result<()> {
        sqlx::query(
            "update folders set uid_validity = $2, last_uid = $3, highest_modseq = $4, synced_at = now() where id = $1",
        )
        .bind(folder.id)
        .bind(state.uid_validity.map(i64::from))
        .bind(last_uid)
        .bind(state.modseq.map(|m| m.to_string()))
        .execute(&self.ctx.db)
        .await?;
        Ok(())
    }

    async fn fetch_rows(
        &self,
        s: &mut ImapSession,
        folder: &Folder,
        uids: &[u32],
    ) -> Result<Vec<MessageRow>> {
        // 1. Metadatos y cabeceras (sin descargar cuerpos ni adjuntos)
        let set = uid_set(uids);
        let fetched: Vec<_> = s
            .uid_fetch(
                &set,
                "(UID FLAGS RFC822.SIZE INTERNALDATE BODYSTRUCTURE BODY.PEEK[HEADER])",
            )
            .await?
            .try_collect()
            .await?;
        let parts: HashMap<u32, parse::TextPart> = fetched
            .iter()
            .filter_map(|f| Some((f.uid?, parse::text_part(f.bodystructure()?)?)))
            .collect();

        // 2. Solo la parte de texto para la vista previa, agrupando por sección
        let mut by_section: HashMap<(String, bool), Vec<u32>> = HashMap::new();
        for (uid, p) in &parts {
            by_section
                .entry((p.section.clone(), p.octets > SNIPPET_FULL_LIMIT))
                .or_default()
                .push(*uid);
        }
        let mut texts: HashMap<u32, String> = HashMap::new();
        for ((section, partial), group) in by_section {
            let query = if partial {
                format!("(UID BODY.PEEK[{section}]<0.{SNIPPET_FULL_LIMIT}>)")
            } else {
                format!("(UID BODY.PEEK[{section}])")
            };
            let path = SectionPath::Part(
                section.split('.').filter_map(|n| n.parse().ok()).collect(),
                None,
            );
            let res: Result<Vec<_>, _> = async {
                s.uid_fetch(uid_set(&group), &query)
                    .await?
                    .try_collect()
                    .await
            }
            .await;
            match res {
                Ok(bodies) => {
                    for f in &bodies {
                        let (Some(uid), Some(data)) = (f.uid, f.section(&path)) else {
                            continue;
                        };
                        if let Some(text) =
                            parts.get(&uid).and_then(|p| parse::decode_text(p, data))
                        {
                            texts.insert(uid, text);
                        }
                    }
                }
                // Sin vista previa antes que perder el mensaje
                Err(e) => {
                    warn!(folder = %folder.path, err = %e, "no se pudo leer el texto para la vista previa")
                }
            }
        }

        Ok(fetched
            .iter()
            .filter_map(|f| {
                let uid = f.uid?;
                Some(parse::to_row(
                    self.account.id,
                    folder.id,
                    uid,
                    flag_strings(f.flags()),
                    f.size.unwrap_or(0),
                    f.internal_date().map(|d| d.with_timezone(&Utc)),
                    f.bodystructure(),
                    f.header().unwrap_or_default(),
                    texts.remove(&uid),
                ))
            })
            .collect())
    }

    async fn insert_rows(&self, rows: &[MessageRow]) -> Result<usize> {
        insert_rows(&self.ctx.db, rows).await
    }
}

/// Inserta mensajes nuevos (IMAP o JMAP); ignora los que ya existen.
pub(super) async fn insert_rows(db: &PgPool, rows: &[MessageRow]) -> Result<usize> {
    let mut n = 0;
    for r in rows {
        let res = sqlx::query(
            "insert into messages (account_id, folder_id, uid, remote_id, message_id, in_reply_to, thread_key, subject,
               from_name, from_address, \"to\", cc, date, snippet, flags, seen, flagged, has_attachments, size)
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
             on conflict (folder_id, uid) do nothing",
        )
        .bind(r.account_id)
        .bind(r.folder_id)
        .bind(r.uid)
        .bind(&r.remote_id)
        .bind(&r.message_id)
        .bind(&r.in_reply_to)
        .bind(&r.thread_key)
        .bind(&r.subject)
        .bind(&r.from_name)
        .bind(&r.from_address)
        .bind(Json(&r.to))
        .bind(Json(&r.cc))
        .bind(r.date)
        .bind(&r.snippet)
        .bind(Json(&r.flags))
        .bind(r.seen)
        .bind(r.flagged)
        .bind(r.has_attachments)
        .bind(r.size as i32)
        .execute(db)
        .await?;
        n += res.rows_affected() as usize;
    }
    Ok(n)
}

#[derive(Default, Debug)]
struct Changes {
    inserted: usize,
    deleted: usize,
    updated: Vec<Uuid>,
}

/// Compacta UIDs en rangos: 1,2,3,7 → "1:3,7"
fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        i += 1;
    }
    out.join(",")
}

fn flag_strings<'a>(flags: impl Iterator<Item = Flag<'a>>) -> Vec<String> {
    flags
        .filter_map(|f| match f {
            Flag::Seen => Some("\\Seen".to_string()),
            Flag::Answered => Some("\\Answered".into()),
            Flag::Flagged => Some("\\Flagged".into()),
            Flag::Deleted => Some("\\Deleted".into()),
            Flag::Draft => Some("\\Draft".into()),
            Flag::Recent => None, // \Recent depende de la sesión: no se guarda
            Flag::MayCreate => None,
            Flag::Custom(c) => Some(c.to_string()),
        })
        .collect()
}

fn sorted(v: &[String]) -> Vec<&str> {
    let mut s: Vec<&str> = v.iter().map(String::as_str).collect();
    s.sort_unstable();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_uid_ranges() {
        assert_eq!(uid_set(&[7, 1, 3, 2]), "1:3,7");
        assert_eq!(uid_set(&[5]), "5");
        assert_eq!(uid_set(&[1, 3, 5, 6]), "1,3,5:6");
    }
}
