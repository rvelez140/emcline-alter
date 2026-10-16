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
    /// Programa el respaldo diario en cron (/etc/cron.d/rvmail) a esta hora (HH:MM) y termina
    #[arg(long, value_name = "HH:MM")]
    pub install_cron: Option<String>,
}

pub async fn backup(a: BackupArgs) -> Result<()> {
    let dir = a.common.dir();
    if let Some(at) = &a.install_cron {
        return install_cron(&dir, at, a.with_mail);
    }
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
        let name = format!("stalwart-{stamp}.tar.gz");
        info("Copiando los buzones y la configuración de Stalwart (se detiene unos segundos)…");
        // RocksDB no se copia en caliente de forma consistente: se para el servicio
        let _ = engine.compose(&dir, &["stop", "stalwart"]);
        let ok_tar = Command::new(engine.bin())
            .args(["run", "--rm"])
            .args(stalwart_mounts(&engine, &env, true))
            .arg("-v")
            .arg(format!("{}:/backup", dest.canonicalize()?.display()))
            .args([
                "docker.io/library/alpine:3",
                "tar",
                "czf",
                &format!("/backup/{name}"),
                "-C",
                "/s",
                ".",
            ])
            .status()
            .is_ok_and(|s| s.success());
        let _ = engine.compose(&dir, &["start", "stalwart"]);
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
    if env.value("RESTIC_REPOSITORY").is_some() {
        offsite(&engine, &env, &dest)?;
    }
    Ok(())
}

/// Nombre real del volumen según el motor (compose antepone el proyecto).
pub fn volume_name(engine: &engine::Engine, env: &EnvFile, name: &str) -> String {
    if engine.quadlet {
        format!("rvmail-{}", name.replace('_', "-"))
    } else {
        // Como compose: la variable del entorno manda sobre la del .env
        let project = std::env::var("COMPOSE_PROJECT_NAME")
            .ok()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| env.get_or("COMPOSE_PROJECT_NAME", "rvmail"));
        format!("{project}_{name}")
    }
}

fn install_cron(dir: &Path, at: &str, with_mail: bool) -> Result<()> {
    let Some((h, m)) = at.split_once(':') else {
        bail!("Hora no válida: {at} (usa HH:MM)")
    };
    let (h, m): (u8, u8) = (h.parse()?, m.parse()?);
    if h > 23 || m > 59 {
        bail!("Hora no válida: {at}");
    }
    let exe = std::env::current_exe()?;
    let dir = dir.canonicalize()?;
    let line = format!(
        "# Respaldo diario de RV Mail (rvmail backup --install-cron)\n\
         SHELL=/bin/sh\nPATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n\
         {m} {h} * * * root {} backup --dir {} {}>> {}/backups/backup.log 2>&1\n",
        exe.display(),
        dir.display(),
        if with_mail { "--with-mail " } else { "" },
        dir.display()
    );
    std::fs::create_dir_all(dir.join("backups"))?;
    std::fs::write("/etc/cron.d/rvmail", line)
        .context("No se pudo escribir /etc/cron.d/rvmail (¿root?)")?;
    ok(format!(
        "Respaldo diario a las {h:02}:{m:02} en /etc/cron.d/rvmail"
    ));
    Ok(())
}

// ---------------------------------------------------------------- restic (fuera del servidor)

const RESTIC_IMAGE: &str = "docker.io/restic/restic:0.17.3";

/// `docker run … restic <args>` con el repositorio y las credenciales del `.env`.
fn restic(
    engine: &engine::Engine,
    env: &EnvFile,
    mounts: &[(String, &str)],
    args: &[&str],
) -> Command {
    let repo = env.get_or("RESTIC_REPOSITORY", "");
    let mut c = Command::new(engine.bin());
    c.args(["run", "--rm", "-i"])
        .args(["-e", "RESTIC_REPOSITORY", "-e", "RESTIC_PASSWORD"])
        .args([
            "-e",
            "AWS_ACCESS_KEY_ID",
            "-e",
            "AWS_SECRET_ACCESS_KEY",
            "-e",
            "AWS_DEFAULT_REGION",
        ])
        .env("RESTIC_REPOSITORY", &repo)
        .env("RESTIC_PASSWORD", env.get_or("RESTIC_PASSWORD", ""))
        .env("AWS_ACCESS_KEY_ID", env.get_or("RESTIC_S3_ACCESS_KEY", ""))
        .env(
            "AWS_SECRET_ACCESS_KEY",
            env.get_or("RESTIC_S3_SECRET_KEY", ""),
        )
        .env(
            "AWS_DEFAULT_REGION",
            env.get_or("RESTIC_S3_REGION", "us-east-1"),
        )
        .args([
            "-v",
            &format!(
                "{}:/root/.cache/restic",
                volume_name(engine, env, "restic_cache")
            ),
        ]);
    for (host, inside) in mounts {
        c.args(["-v", &format!("{host}:{inside}")]);
    }
    c.arg(RESTIC_IMAGE);
    // Oracle Object Storage (API compatible con S3) necesita direcciones "path-style"
    if repo.contains("oraclecloud.com") {
        c.args(["-o", "s3.bucket-lookup=path"]);
    }
    c.args(args);
    c
}

fn run_restic(mut c: Command, what: &str) -> Result<()> {
    let ok = c.status().is_ok_and(|s| s.success());
    if !ok {
        bail!("restic: falló {what}");
    }
    Ok(())
}

/// Sube los respaldos locales al repositorio restic (cifrado) y aplica la retención.
pub fn offsite(engine: &engine::Engine, env: &EnvFile, dest: &Path) -> Result<()> {
    if env.value("RESTIC_PASSWORD").is_none() {
        bail!("Falta RESTIC_PASSWORD en .env");
    }
    let mount = vec![(dest.canonicalize()?.display().to_string(), "/backup:ro")];
    let initialized = restic(engine, env, &[], &["cat", "config"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !initialized {
        info("Creando el repositorio restic…");
        run_restic(restic(engine, env, &[], &["init"]), "init")?;
    }
    info("Subiendo el respaldo fuera del servidor (restic)…");
    run_restic(
        restic(
            engine,
            env,
            &mount,
            &["backup", "--host", "rvmail", "--tag", "rvmail", "/backup"],
        ),
        "backup",
    )?;
    let keep: Vec<String> = env
        .get_or("RESTIC_KEEP", "7,4,6")
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let (d, w, m) = (
        keep.first().map_or("7", String::as_str),
        keep.get(1).map_or("4", String::as_str),
        keep.get(2).map_or("6", String::as_str),
    );
    run_restic(
        restic(
            engine,
            env,
            &[],
            &[
                "forget",
                "--tag",
                "rvmail",
                "--keep-daily",
                d,
                "--keep-weekly",
                w,
                "--keep-monthly",
                m,
                "--prune",
            ],
        ),
        "forget",
    )?;
    ok("Respaldo cifrado subido (restic)");
    Ok(())
}

/// Recupera los buzones de Stalwart del último respaldo de restic (failover).
pub fn restore_stalwart_from_restic(
    engine: &engine::Engine,
    dir: &Path,
    env: &EnvFile,
) -> Result<()> {
    if env.value("RESTIC_REPOSITORY").is_none() {
        bail!("No hay RESTIC_REPOSITORY configurado: no se pueden recuperar los buzones");
    }
    let tmp = dir.join("backups/restore");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mount = vec![(tmp.canonicalize()?.display().to_string(), "/restore")];
    info("Descargando el último respaldo de restic…");
    run_restic(
        restic(
            engine,
            env,
            &mount,
            &[
                "restore", "latest", "--tag", "rvmail", "--target", "/restore",
            ],
        ),
        "restore",
    )?;
    let newest = std::fs::read_dir(tmp.join("backup"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("stalwart-") && n.ends_with(".tar.gz"))
        .max()
        .context("El respaldo no incluye buzones de Stalwart (usa rvmail backup --with-mail)")?;
    info(format!("Restaurando {newest}…"));
    let ok_tar = Command::new(engine.bin())
        .args(["run", "--rm"])
        .args(stalwart_mounts(engine, env, false))
        .arg("-v")
        .arg(format!("{}:/backup:ro", tmp.join("backup").display()))
        .args(["docker.io/library/alpine:3", "sh", "-c"])
        .arg(format!(
            "find /s/data /s/etc -mindepth 1 -delete && tar xzf /backup/{newest} -C /s"
        ))
        .status()
        .is_ok_and(|s| s.success());
    let _ = std::fs::remove_dir_all(&tmp);
    if !ok_tar {
        bail!("No se pudieron restaurar los buzones");
    }
    ok("Buzones de Stalwart restaurados");
    Ok(())
}

/// Volúmenes de Stalwart montados en /s/data (buzones) y /s/etc (configuración).
fn stalwart_mounts(engine: &engine::Engine, env: &EnvFile, read_only: bool) -> Vec<String> {
    let ro = if read_only { ":ro" } else { "" };
    vec![
        "-v".into(),
        format!("{}:/s/data{ro}", volume_name(engine, env, "stalwart_data")),
        "-v".into(),
        format!("{}:/s/etc{ro}", volume_name(engine, env, "stalwart_etc")),
    ]
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
    #[arg(required_unless_present = "mail")]
    pub file: Option<PathBuf>,
    /// Restaura los buzones de Stalwart del último respaldo de restic
    #[arg(long)]
    pub mail: bool,
}

pub async fn restore(a: RestoreArgs) -> Result<()> {
    let ui = a.common.ui();
    let dir = a.common.dir();
    let env = EnvFile::load(&dir.join(".env"))?;
    if a.mail {
        if !a.common.yes
            && !ui.confirm(
                "Esto reemplaza los buzones actuales de Stalwart. ¿Continuar?",
                false,
            )?
        {
            bail!("Cancelado");
        }
        let engine = engine::Engine::detect("")?;
        engine.compose(&dir, &["stop", "stalwart"])?;
        let res = restore_stalwart_from_restic(&engine, &dir, &env);
        engine.compose(&dir, &["start", "stalwart"])?;
        res?;
        if a.file.is_none() {
            return Ok(());
        }
    }
    let file = a.file.clone().context("Indica el archivo .dump")?;
    if !file.exists() {
        bail!("No existe {}", file.display());
    }
    warn("El .env debe tener la MISMA MASTER_KEY que cuando se hizo el respaldo.");
    let sure = a.common.yes
        || ui.confirm(
            &format!(
                "Esto reemplaza la base de datos actual con {}. ¿Continuar?",
                file.display()
            ),
            false,
        )?;
    if !sure {
        bail!("Cancelado");
    }
    let engine = engine::Engine::detect("")?;
    engine.compose(&dir, &["stop", "app", "worker"])?;
    let input = std::fs::File::open(&file)?;
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
            install_cron: None,
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
