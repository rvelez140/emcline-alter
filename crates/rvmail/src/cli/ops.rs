//! `rvmail update`, `rvmail backup` y `rvmail restore`.

use super::envfile::{set_private, EnvFile};
use super::{assets, engine, info, ok, warn, Common};
use anyhow::{bail, Context, Result};
use clap::Args;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ---------------------------------------------------------------- backup

#[derive(Args)]
pub struct BackupArgs {
    #[command(flatten)]
    pub common: Common,
    /// Carpeta de destino (por defecto <instalación>/backups)
    #[arg(long)]
    pub dest: Option<PathBuf>,
    /// Borra respaldos con más de N días
    #[arg(long, default_value_t = 14, env = "KEEP_DAYS")]
    pub keep_days: u64,
    /// Incluye los buzones de Stalwart (volumen de datos)
    #[arg(long)]
    pub with_mail: bool,
}

pub async fn backup(a: BackupArgs) -> Result<()> {
    let dir = a.common.dir();
    let env = EnvFile::load(&dir.join(".env"))?;
    let engine = engine::Engine::detect("")?;
    let dest = a.dest.unwrap_or_else(|| dir.join("backups"));
    std::fs::create_dir_all(&dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o700))?;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dump = dest.join(format!("rvmail-{stamp}.dump"));
    let user = env.get_or("POSTGRES_USER", "rvmail");
    let db = env.get_or("POSTGRES_DB", "rvmail");

    let file = std::fs::File::create(&dump)?;
    set_private(&dump)?;
    let status = engine
        .exec_cmd(
            &dir,
            "postgres",
            &[
                "pg_dump",
                "-U",
                &user,
                "-d",
                &db,
                "--format=custom",
                "--compress=6",
            ],
        )
        .stdout(file)
        .status()
        .context("No se pudo ejecutar pg_dump")?;
    if !status.success() {
        let _ = std::fs::remove_file(&dump);
        bail!("pg_dump falló (¿está corriendo postgres?)");
    }
    let env_copy = dest.join(format!("env-{stamp}.bak"));
    std::fs::copy(dir.join(".env"), &env_copy)?;
    set_private(&env_copy)?;

    if a.with_mail && env.get_or("MAILSERVER_PROVIDER", "") == "stalwart" {
        let volume = if engine.quadlet {
            "rvmail-stalwart-data"
        } else {
            "rvmail_stalwart_data"
        };
        let name = format!("stalwart-{stamp}.tar.gz");
        info("Copiando los buzones de Stalwart…");
        let ok_tar = Command::new(engine.bin())
            .args(["run", "--rm", "-v", &format!("{volume}:/data:ro"), "-v"])
            .arg(format!("{}:/backup", dest.canonicalize()?.display()))
            .args([
                "docker.io/library/alpine:3",
                "tar",
                "czf",
                &format!("/backup/{name}"),
                "-C",
                "/data",
                ".",
            ])
            .status()
            .is_ok_and(|s| s.success());
        if ok_tar {
            set_private(&dest.join(&name))?;
        } else {
            warn("No se pudo copiar el volumen de Stalwart");
        }
    }

    prune(&dest, a.keep_days);
    let size = std::fs::metadata(&dump).map(|m| m.len()).unwrap_or(0);
    ok(format!(
        "Respaldo: {} ({:.1} MB)",
        dump.display(),
        size as f64 / 1_048_576.0
    ));
    Ok(())
}

fn prune(dest: &Path, keep_days: u64) {
    let max = std::time::Duration::from_secs(keep_days * 86_400);
    let Ok(rd) = std::fs::read_dir(dest) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let ours = (name.starts_with("rvmail-") && name.ends_with(".dump"))
            || (name.starts_with("env-") && name.ends_with(".bak"))
            || (name.starts_with("stalwart-") && name.ends_with(".tar.gz"));
        let old = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max);
        if ours && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// ---------------------------------------------------------------- restore

#[derive(Args)]
pub struct RestoreArgs {
    #[command(flatten)]
    pub common: Common,
    /// Archivo .dump creado por `rvmail backup`
    pub file: PathBuf,
}

pub async fn restore(a: RestoreArgs) -> Result<()> {
    let ui = a.common.ui();
    let dir = a.common.dir();
    if !a.file.exists() {
        bail!("No existe {}", a.file.display());
    }
    let env = EnvFile::load(&dir.join(".env"))?;
    warn("El .env debe tener la MISMA MASTER_KEY que cuando se hizo el respaldo.");
    let sure = a.common.yes
        || ui.confirm(
            &format!(
                "Esto reemplaza la base de datos actual con {}. ¿Continuar?",
                a.file.display()
            ),
            false,
        )?;
    if !sure {
        bail!("Cancelado");
    }
    let engine = engine::Engine::detect("")?;
    engine.compose(&dir, &["stop", "app", "worker"])?;
    let input = std::fs::File::open(&a.file)?;
    let user = env.get_or("POSTGRES_USER", "rvmail");
    let db = env.get_or("POSTGRES_DB", "rvmail");
    let status = engine
        .exec_cmd(
            &dir,
            "postgres",
            &[
                "pg_restore",
                "-U",
                &user,
                "-d",
                &db,
                "--clean",
                "--if-exists",
                "--no-owner",
            ],
        )
        .stdin(input)
        .status()?;
    engine.compose(&dir, &["start", "app", "worker"])?;
    if !status.success() {
        bail!("pg_restore terminó con errores (revisa la salida de arriba)");
    }
    if engine::wait_healthy(&env.get_or("HTTP_PORT", "8088"), 120).await {
        ok("Restauración completada");
        Ok(())
    } else {
        bail!("La app no responde tras restaurar: rvmail doctor")
    }
}

// ---------------------------------------------------------------- update

#[derive(Args)]
pub struct UpdateArgs {
    #[command(flatten)]
    pub common: Common,
    /// No actualizar el propio binario `rvmail`
    #[arg(long)]
    pub no_self: bool,
    /// No hacer respaldo antes de actualizar
    #[arg(long)]
    pub no_backup: bool,
}

pub async fn update(a: UpdateArgs) -> Result<()> {
    let dir = a.common.dir();
    let in_repo = dir.join(".git").exists();
    if !a.no_self && !in_repo {
        // La ruta se toma antes de reemplazar el archivo: después /proc/self/exe apunta al borrado
        let exe = std::env::current_exe()?;
        match self_update(&exe).await {
            Ok(Some(v)) => {
                ok(format!(
                    "rvmail actualizado a {v}; continúo con la nueva versión"
                ));
                let mut args: Vec<String> = std::env::args().skip(1).collect();
                args.push("--no-self".into());
                let status = Command::new(exe).args(args).status()?;
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(None) => info(format!(
                "rvmail {} es la última versión",
                env!("CARGO_PKG_VERSION")
            )),
            Err(e) => warn(format!("No se pudo comprobar si hay un binario nuevo: {e}")),
        }
    }
    if !a.no_backup {
        info("Respaldo previo…");
        backup(BackupArgs {
            common: a.common.clone(),
            dest: None,
            keep_days: 14,
            with_mail: false,
        })
        .await?;
    }
    if in_repo {
        info("Descargando cambios del repositorio…");
        let ok_pull = Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(&dir)
            .status()
            .is_ok_and(|s| s.success());
        if !ok_pull {
            bail!("git pull falló (¿cambios locales?)");
        }
    } else {
        let written = assets::write(&dir, true)?;
        if !written.is_empty() {
            info(format!(
                "Archivos de despliegue actualizados: {}",
                written.join(", ")
            ));
        }
    }
    let env = EnvFile::load(&dir.join(".env"))?;
    let engine = engine::Engine::detect("")?;
    super::install::start(&engine, &dir, &env.get_or("IMAGE_PREFIX", "rvmail")).await?;
    if !engine.quadlet {
        let _ = Command::new(engine.bin())
            .args(["image", "prune", "-f"])
            .stdout(Stdio::null())
            .status();
    }
    ok("RV Mail actualizado");
    Ok(())
}

fn release_target() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        _ => "x86_64",
    }
}

/// Descarga el último binario publicado; devuelve la versión nueva si cambió.
async fn self_update(exe: &Path) -> Result<Option<String>> {
    let repo = std::env::var("RVMAIL_REPO").unwrap_or_else(|_| super::DEFAULT_REPO.into());
    let base = std::env::var("RVMAIL_BASE_URL")
        .unwrap_or_else(|_| format!("https://github.com/{repo}/releases/latest/download"));
    let asset = format!("rvmail-{}-linux", release_target());
    let http = crate::net::tls::http_client(false);
    let sums = http
        .get(format!("{base}/SHA256SUMS"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let expected = sums
        .lines()
        .find_map(|l| {
            l.split_once("  ")
                .filter(|(_, n)| n.trim() == asset)
                .map(|(h, _)| h.to_string())
        })
        .context("La versión publicada no trae este binario")?;
    let current = sha2::Sha256::digest(std::fs::read(exe)?);
    if hex::encode(current) == expected {
        return Ok(None);
    }
    let bytes = http
        .get(format!("{base}/{asset}"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if hex::encode(sha2::Sha256::digest(&bytes)) != expected {
        bail!("El binario descargado no coincide con SHA256SUMS");
    }
    let tmp = exe.with_extension("new");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    let version = engine::output(&tmp.to_string_lossy(), &["--version"]);
    if version.is_empty() {
        let _ = std::fs::remove_file(&tmp);
        bail!("El binario descargado no arranca en este sistema");
    }
    std::fs::rename(&tmp, exe)?;
    Ok(Some(version))
}
