//! RV Mail — binario único: `rvmail worker`, `rvmail api`, `rvmail install`…

mod net;
mod worker;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "rvmail",
    version,
    about = "RV Mail — correo multi-cuenta autoalojable"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sincronizador IMAP de todas las cuentas
    Worker,
    /// Comprobación de salud para Docker/Podman (código de salida 0 = sano)
    Healthcheck {
        #[command(subcommand)]
        what: Health,
    },
}

#[derive(Subcommand)]
enum Health {
    /// El worker escribió su latido hace menos de 90 s
    Worker,
}

fn init_logs() {
    let filter = EnvFilter::try_from_env("LOG_LEVEL")
        .or_else(|_| {
            EnvFilter::try_new(std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".into()))
        })
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("LOG_FORMAT")
        .map(|v| v == "json")
        .unwrap_or(false);
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Healthcheck { what } = &cli.cmd {
        std::process::exit(healthcheck(what));
    }
    init_logs();
    match cli.cmd {
        Cmd::Worker => worker::run().await,
        Cmd::Healthcheck { .. } => unreachable!(),
    }
}

fn healthcheck(what: &Health) -> i32 {
    match what {
        Health::Worker => {
            let cfg = rvmail_core::config::WorkerConfig::from_env();
            let fresh = std::fs::metadata(&cfg.heartbeat_file)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() < 90);
            if fresh {
                0
            } else {
                1
            }
        }
    }
}
