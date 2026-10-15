//! Sincronizador IMAP multi-cuenta (`rvmail worker`).
//!
//! Cada cuenta corre en su propia tarea tokio; un lease en Redis garantiza
//! que solo un worker la sincroniza (se puede escalar con réplicas).
//! Mismo contrato que `apps/worker` (Node): BD, claves Redis y eventos.

mod lease;
pub mod parse;
mod session;
pub mod special_use;

use anyhow::Result;
use futures::StreamExt;
use lease::LeaseManager;
use redis::aio::ConnectionManager;
use rvmail_core::config::{CoreConfig, WorkerConfig};
use rvmail_core::events::{WorkerControl, WORKER_CONTROL_CHANNEL};
use session::{AccountSession, Command, Ctx};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

const SUPERVISE_EVERY: Duration = Duration::from_secs(30);
const RENEW_EVERY: Duration = Duration::from_secs(20);

struct Running {
    tx: mpsc::Sender<Command>,
    task: JoinHandle<()>,
}

struct Supervisor {
    ctx: Ctx,
    leases: LeaseManager,
    sessions: HashMap<Uuid, Running>,
}

impl Supervisor {
    async fn start_account(&mut self, id: Uuid) -> Result<()> {
        if self.sessions.contains_key(&id) || self.sessions.len() >= self.ctx.cfg.max_accounts {
            return Ok(());
        }
        if !self.leases.acquire(id).await? {
            return Ok(()); // otro worker la tiene
        }
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(AccountSession::new(id, self.ctx.clone(), rx).run());
        self.sessions.insert(id, Running { tx, task });
        info!(account = %id, "cuenta asignada a este worker");
        Ok(())
    }

    async fn stop_account(&mut self, id: Uuid) {
        if let Some(r) = self.sessions.remove(&id) {
            let _ = r.tx.send(Command::Stop).await;
            // Espera breve a que cierre la sesión IMAP de forma ordenada
            if tokio::time::timeout(Duration::from_secs(10), r.task)
                .await
                .is_err()
            {
                warn!(account = %id, "la sesión no cerró a tiempo");
            }
            if let Err(e) = self.leases.release(id).await {
                warn!(err = %e, "no se pudo liberar el lease");
            }
        }
    }

    /// Toma cuentas libres y suelta las que ya no corresponden.
    async fn supervise(&mut self) -> Result<()> {
        // Sesiones que terminaron solas (cuenta deshabilitada o error de autenticación)
        let finished: Vec<Uuid> = self
            .sessions
            .iter()
            .filter(|(_, r)| r.task.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            self.sessions.remove(&id);
            let _ = self.leases.release(id).await;
        }

        let wanted: Vec<Uuid> = sqlx::query_scalar(
            "select id from mail_accounts where enabled = true and status <> 'auth_error'",
        )
        .fetch_all(&self.ctx.db)
        .await?;
        let current: Vec<Uuid> = self.sessions.keys().copied().collect();
        for id in current {
            if !wanted.contains(&id) {
                self.stop_account(id).await;
            }
        }
        for id in wanted {
            self.start_account(id).await?;
        }
        tokio::fs::write(
            &self.ctx.cfg.heartbeat_file,
            chrono::Utc::now().timestamp_millis().to_string(),
        )
        .await
        .ok();
        Ok(())
    }

    async fn renew_leases(&mut self) {
        let ids: Vec<Uuid> = self.sessions.keys().copied().collect();
        for id in ids {
            match self.leases.renew(id).await {
                Ok(true) => {}
                Ok(false) => {
                    warn!(account = %id, "lease perdido, soltando cuenta");
                    if let Some(r) = self.sessions.remove(&id) {
                        let _ = r.tx.send(Command::Stop).await;
                    }
                }
                Err(e) => warn!(err = %e, "error renovando lease"),
            }
        }
    }

    async fn on_control(&mut self, msg: WorkerControl) -> Result<()> {
        match msg {
            WorkerControl::AccountSync { account_id } => {
                if let Some(r) = self.sessions.get(&account_id) {
                    let _ = r.tx.try_send(Command::Sync);
                }
            }
            WorkerControl::AccountChanged { account_id } => {
                // Cambió la cuenta: reiniciar la sesión (si la tenía) o intentar tomarla
                self.stop_account(account_id).await;
                self.start_account(account_id).await?;
            }
            WorkerControl::AccountDeleted { account_id } => self.stop_account(account_id).await,
        }
        Ok(())
    }
}

pub async fn run() -> Result<()> {
    let core = CoreConfig::from_env()?;
    let cfg = WorkerConfig::from_env();
    let db = rvmail_core::db::connect(&core.database_url, cfg.db_pool_size).await?;
    let redis_client = redis::Client::open(core.redis_url.as_str())?;
    let redis = ConnectionManager::new(redis_client.clone()).await?;
    let owner = format!(
        "{}:{}",
        std::env::var("HOSTNAME").unwrap_or_else(|_| "rvmail".into()),
        std::process::id()
    );
    info!(%owner, ?cfg, "worker iniciado");

    let ctx = Ctx {
        db,
        redis: redis.clone(),
        core,
        cfg,
    };
    let mut sup = Supervisor {
        leases: LeaseManager::new(redis, owner),
        ctx,
        sessions: HashMap::new(),
    };

    let mut pubsub = redis_client.get_async_pubsub().await?;
    pubsub.subscribe(WORKER_CONTROL_CHANNEL).await?;
    let mut control = pubsub.into_on_message();

    sup.supervise().await?;
    let mut supervise = tokio::time::interval(SUPERVISE_EVERY);
    let mut renew = tokio::time::interval(RENEW_EVERY);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        tokio::select! {
            _ = supervise.tick() => if let Err(e) = sup.supervise().await { error!(err = %format!("{e:#}"), "supervise") },
            _ = renew.tick() => sup.renew_leases().await,
            msg = control.next() => {
                let Some(msg) = msg else { anyhow::bail!("se perdió la suscripción de control en Redis") };
                let payload: String = msg.get_payload().unwrap_or_default();
                match serde_json::from_str::<WorkerControl>(&payload) {
                    Ok(m) => if let Err(e) = sup.on_control(m).await { error!(err = %format!("{e:#}"), "control") },
                    Err(_) => warn!("mensaje de control no válido"),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
        }
    }

    info!("cerrando worker");
    let ids: Vec<Uuid> = sup.sessions.keys().copied().collect();
    for id in ids {
        sup.stop_account(id).await;
    }
    Ok(())
}
