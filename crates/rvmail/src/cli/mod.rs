//! Administración del servidor desde la terminal del VPS:
//! `rvmail install | config | doctor | update | backup | restore`.
//!
//! Se ejecuta en el host (no en el contenedor) y maneja la instalación con
//! Docker, Podman (`podman compose`) o Podman Quadlet + systemd.

pub mod assets;
pub mod config;
pub mod detect;
pub mod doctor;
pub mod engine;
pub mod envfile;
pub mod install;
pub mod ops;
pub mod quadlet;
pub mod stalwart_setup;

use anyhow::Result;
use clap::{Args, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

pub const DEFAULT_DIR: &str = "/opt/rvmail";
/// Repositorio de GitHub de las versiones publicadas (binarios e imágenes).
pub const DEFAULT_REPO: &str = "rvelez140/emcline-alter";
pub const DEFAULT_IMAGE: &str = "ghcr.io/rvelez140/rvmail";

#[derive(Args, Clone)]
pub struct Common {
    /// Carpeta de la instalación (por defecto la actual si tiene docker-compose.yml, si no /opt/rvmail)
    #[arg(long, global = true, env = "RVMAIL_DIR")]
    pub dir: Option<PathBuf>,
    /// No preguntar: usar las opciones dadas y los valores por defecto
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
}

impl Common {
    pub fn dir(&self) -> PathBuf {
        if let Some(d) = &self.dir {
            return d.clone();
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if is_install_dir(&cwd) {
            cwd
        } else {
            PathBuf::from(DEFAULT_DIR)
        }
    }

    pub fn ui(&self) -> Ui {
        Ui {
            interactive: !self.yes && std::io::stdin().is_terminal(),
        }
    }
}

fn is_install_dir(p: &Path) -> bool {
    std::fs::read_to_string(p.join("docker-compose.yml")).is_ok_and(|c| c.contains("name: rvmail"))
}

#[derive(Subcommand)]
pub enum AdminCmd {
    /// Asistente de instalación con autodetección (dominio, proxy, servidor de correo, SSO…)
    Install(Box<install::InstallArgs>),
    /// Ver o cambiar la configuración después de instalar
    Config(config::ConfigArgs),
    /// Diagnóstico: DNS, TLS, puertos, servidor de correo, respaldos
    Doctor(doctor::DoctorArgs),
    /// Actualiza RV Mail (binario, imágenes y servicios) con respaldo previo
    Update(ops::UpdateArgs),
    /// Respaldo de la base de datos, el .env y (opcional) los buzones de Stalwart
    Backup(ops::BackupArgs),
    /// Restaura un respaldo de la base de datos
    Restore(ops::RestoreArgs),
}

pub async fn run(cmd: AdminCmd) -> Result<()> {
    match cmd {
        AdminCmd::Install(a) => install::run(*a).await,
        AdminCmd::Config(a) => config::run(a).await,
        AdminCmd::Doctor(a) => doctor::run(a).await,
        AdminCmd::Update(a) => ops::update(a).await,
        AdminCmd::Backup(a) => ops::backup(a).await,
        AdminCmd::Restore(a) => ops::restore(a).await,
    }
}

// ---------------------------------------------------------------- salida y preguntas

fn color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, mark: &str, msg: &str) {
    if color() {
        println!("\x1b[{code}m{mark} {msg}\x1b[0m");
    } else {
        println!("{mark} {msg}");
    }
}

pub fn ok(msg: impl AsRef<str>) {
    paint("32", "✔", msg.as_ref());
}
pub fn info(msg: impl AsRef<str>) {
    paint("36", "➜", msg.as_ref());
}
pub fn warn(msg: impl AsRef<str>) {
    paint("33", "!", msg.as_ref());
}
pub fn fail(msg: impl AsRef<str>) {
    paint("31", "✘", msg.as_ref());
}
pub fn title(msg: impl AsRef<str>) {
    if color() {
        println!("\n\x1b[1m{}\x1b[0m", msg.as_ref());
    } else {
        println!("\n{}", msg.as_ref());
    }
}

/// Preguntas interactivas; sin terminal (o con `--yes`) devuelven el valor por defecto.
#[derive(Clone, Copy)]
pub struct Ui {
    pub interactive: bool,
}

impl Ui {
    pub fn input(&self, prompt: &str, default: &str) -> Result<String> {
        if !self.interactive {
            return Ok(default.to_string());
        }
        let mut q = dialoguer::Input::<String>::new()
            .with_prompt(prompt)
            .allow_empty(true);
        if !default.is_empty() {
            q = q.default(default.to_string());
        }
        Ok(q.interact_text()?.trim().to_string())
    }

    pub fn password(&self, prompt: &str) -> Result<String> {
        if !self.interactive {
            return Ok(String::new());
        }
        Ok(dialoguer::Password::new()
            .with_prompt(prompt)
            .allow_empty_password(true)
            .interact()?)
    }

    pub fn confirm(&self, prompt: &str, default: bool) -> Result<bool> {
        if !self.interactive {
            return Ok(default);
        }
        Ok(dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(default)
            .interact()?)
    }

    /// Índice de la opción elegida.
    pub fn select(&self, prompt: &str, items: &[String], default: usize) -> Result<usize> {
        if !self.interactive {
            return Ok(default);
        }
        Ok(dialoguer::Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(default)
            .interact()?)
    }
}

/// ¿Somos root? (necesario para escribir en /opt, /etc y usar Docker sin sudo).
pub fn is_root() -> bool {
    engine::output("id", &["-u"]) == "0"
}
