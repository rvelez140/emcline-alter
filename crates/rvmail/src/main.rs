//! RV Mail — binario único: `rvmail worker`, `rvmail api`, `rvmail install`…

mod api;
mod net;
mod oauth;
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
    /// API HTTP (REST, tiempo real, MCP) y web
    Api,
    /// Aplica las migraciones de la base de datos y termina
    Migrate,
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
    /// La API responde 200 en /api/health
    Api,
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
        Cmd::Api => api::run().await,
        Cmd::Migrate => {
            let core = rvmail_core::config::CoreConfig::from_env()?;
            let db = rvmail_core::db::connect(&core.database_url, 2).await?;
            api::migrate::run(&db).await?;
            tracing::info!("migraciones aplicadas");
            Ok(())
        }
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
        Health::Api => {
            use std::io::{Read, Write};
            let port = std::env::var("API_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000u16);
            let check = || -> std::io::Result<bool> {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                let mut s =
                    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3))?;
                s.set_read_timeout(Some(std::time::Duration::from_secs(4)))?;
                s.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
                let mut buf = [0u8; 64];
                let n = s.read(&mut buf)?;
                Ok(buf[..n].starts_with(b"HTTP/1.0 200") || buf[..n].starts_with(b"HTTP/1.1 200"))
            };
            i32::from(!check().unwrap_or(false))
        }
    }
}
