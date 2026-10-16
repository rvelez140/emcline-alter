//! Alta disponibilidad: servidor principal + respaldo (p. ej. un VPS ARM de Oracle).
//!
//! - `rvmail ha init` (principal): red privada WireGuard, rol `replicator`, slot de
//!   replicación y un paquete (`ha/standby-bundle.json`, 600) para el respaldo.
//! - `rvmail ha join --bundle …` (respaldo): WireGuard, copia base con
//!   `pg_basebackup` y Postgres en réplica de solo lectura (streaming).
//! - `rvmail ha status`: estado y retraso de la réplica.
//! - `rvmail failover promote` (respaldo): lo convierte en principal.

use super::envfile::{gen_hex, set_private, EnvFile};
use super::{assets, detect, engine, info, ok, title, warn, Common};
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const WG_IFACE: &str = "rvmail0";
pub const WG_CONF: &str = "/etc/wireguard/rvmail0.conf";
pub const PRIMARY_WG_IP: &str = "10.57.0.1";
pub const STANDBY_WG_IP: &str = "10.57.0.2";
pub const SLOT: &str = "rvmail_standby";
pub const HA_COMPOSE: &str = "docker-compose.yml:docker-compose.ha.yml";

#[derive(Args)]
pub struct HaArgs {
    #[command(flatten)]
    pub common: Common,
    #[command(subcommand)]
    pub cmd: HaCmd,
}

#[derive(Subcommand)]
pub enum HaCmd {
    /// En el PRINCIPAL: prepara la réplica y genera el paquete para el respaldo
    Init {
        /// IP o nombre público del principal para WireGuard (por defecto, la IP pública detectada)
        #[arg(long)]
        endpoint: Option<String>,
        /// Puerto UDP de WireGuard
        #[arg(long, default_value_t = 51820)]
        wg_port: u16,
        /// Usar una red privada que ya existe (VCN, Tailscale…) en vez de WireGuard:
        /// IP privada del principal donde escuchará Postgres
        #[arg(long)]
        via: Option<String>,
    },
    /// En el RESPALDO: se une al principal con el paquete generado por `ha init`
    Join {
        /// Ruta del paquete copiado del principal
        #[arg(long)]
        bundle: PathBuf,
    },
    /// Estado de la replicación
    Status,
}

#[derive(Args)]
pub struct FailoverArgs {
    #[command(flatten)]
    pub common: Common,
    #[command(subcommand)]
    pub cmd: FailoverCmd,
}

#[derive(Subcommand)]
pub enum FailoverCmd {
    /// En el RESPALDO: lo convierte en principal (Postgres pasa a lectura/escritura y arranca todo)
    Promote {
        /// Restaura los buzones de Stalwart del último respaldo de restic
        #[arg(long)]
        restore_mail: bool,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Bundle {
    pub version: u32,
    /// Host y puerto donde el respaldo encuentra Postgres del principal
    pub replication_host: String,
    pub replication_port: u16,
    pub replication_password: String,
    /// IP privada propia del respaldo (para publicar su Postgres tras una promoción)
    pub standby_bind: String,
    pub wireguard: Option<WgPeer>,
    /// `.env` del principal (secretos incluidos: MASTER_KEY, contraseñas…)
    pub env: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize)]
pub struct WgPeer {
    pub endpoint: String,
    pub primary_public_key: String,
    pub standby_private_key: String,
}

pub async fn run(a: HaArgs) -> Result<()> {
    let dir = a.common.dir();
    match a.cmd {
        HaCmd::Init {
            endpoint,
            wg_port,
            via,
        } => init(&dir, endpoint, wg_port, via).await,
        HaCmd::Join { bundle } => join(&dir, &bundle).await,
        HaCmd::Status => {
            let st = status(&dir)?;
            let line = format!("Rol {} · {}", st.role, st.detail);
            if st.healthy {
                ok(line);
            } else {
                warn(line);
            }
            if let Some(h) = wg_handshake_age() {
                info(format!("WireGuard: último contacto hace {h} s"));
            }
            Ok(())
        }
    }
}

pub async fn failover(a: FailoverArgs) -> Result<()> {
    let dir = a.common.dir();
    let ui = a.common.ui();
    match a.cmd {
        FailoverCmd::Promote { restore_mail } => {
            promote(&dir, ui, a.common.yes, restore_mail).await
        }
    }
}

// ---------------------------------------------------------------- Postgres

fn db_names(env: &EnvFile) -> (String, String) {
    (
        env.get_or("POSTGRES_USER", "rvmail"),
        env.get_or("POSTGRES_DB", "rvmail"),
    )
}

/// Ejecuta SQL con psql dentro del contenedor de Postgres; devuelve la salida sin adornos.
pub fn psql(engine: &engine::Engine, dir: &Path, env: &EnvFile, sql: &str) -> Result<String> {
    let (user, db) = db_names(env);
    let out = engine
        .exec_cmd(
            dir,
            "postgres",
            &[
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-U",
                &user,
                "-d",
                &db,
                "-tAc",
                sql,
            ],
        )
        .stderr(Stdio::piped())
        .output()
        .context("No se pudo ejecutar psql")?;
    if !out.status.success() {
        bail!("Postgres: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn sql_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// ---------------------------------------------------------------- WireGuard

fn ensure_wireguard() -> Result<()> {
    if engine::has("wg") && Path::new("/usr/bin/wg-quick").exists() {
        return Ok(());
    }
    info("Instalando WireGuard…");
    let script = "if command -v apt-get >/dev/null; then apt-get update -qq && apt-get install -y -qq wireguard-tools; \
                  elif command -v dnf >/dev/null; then dnf install -y -q wireguard-tools; \
                  elif command -v yum >/dev/null; then yum install -y -q wireguard-tools; else exit 1; fi";
    let ok = Command::new("sh")
        .args(["-c", script])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        bail!("No se pudo instalar wireguard-tools; instálalo y repite.");
    }
    Ok(())
}

fn wg_genkey() -> Result<String> {
    let k = engine::output("wg", &["genkey"]);
    if k.is_empty() {
        bail!("`wg genkey` falló");
    }
    Ok(k)
}

fn wg_pubkey(private: &str) -> Result<String> {
    let mut child = Command::new("wg")
        .arg("pubkey")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("stdin")?
        .write_all(private.as_bytes())?;
    let out = child.wait_with_output()?;
    let k = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if k.is_empty() {
        bail!("`wg pubkey` falló");
    }
    Ok(k)
}

/// Clave privada de una configuración existente (para no rotarla en cada `ha init`).
fn existing_key(conf: &str, field: &str) -> Option<String> {
    conf.lines()
        .find_map(|l| l.trim().strip_prefix(field))
        .map(|v| v.trim_start_matches([' ', '=']).trim().to_string())
}

pub fn wg_primary_conf(private: &str, port: u16, standby_pub: &str) -> String {
    format!(
        "# Generado por `rvmail ha init` (principal)\n[Interface]\nAddress = {PRIMARY_WG_IP}/24\nListenPort = {port}\nPrivateKey = {private}\n\n\
         [Peer]\n# respaldo\nPublicKey = {standby_pub}\nAllowedIPs = {STANDBY_WG_IP}/32\n"
    )
}

pub fn wg_standby_conf(private: &str, primary_pub: &str, endpoint: &str) -> String {
    format!(
        "# Generado por `rvmail ha join` (respaldo)\n[Interface]\nAddress = {STANDBY_WG_IP}/24\nPrivateKey = {private}\n\n\
         [Peer]\n# principal\nPublicKey = {primary_pub}\nEndpoint = {endpoint}\nAllowedIPs = {PRIMARY_WG_IP}/32\nPersistentKeepalive = 25\n"
    )
}

fn write_wg(conf: &str) -> Result<()> {
    std::fs::create_dir_all("/etc/wireguard")?;
    std::fs::write(WG_CONF, conf)?;
    set_private(Path::new(WG_CONF))?;
    // Docker puede publicar el puerto en la IP de WireGuard aunque la interfaz aún no exista
    std::fs::write(
        "/etc/sysctl.d/99-rvmail.conf",
        "net.ipv4.ip_nonlocal_bind = 1\n",
    )?;
    let _ = Command::new("sysctl")
        .args(["-q", "-w", "net.ipv4.ip_nonlocal_bind=1"])
        .status();
    // …y además arranca después del túnel
    if Path::new("/etc/systemd/system").exists() {
        std::fs::create_dir_all("/etc/systemd/system/docker.service.d")?;
        std::fs::write(
            "/etc/systemd/system/docker.service.d/rvmail-wireguard.conf",
            format!(
                "[Unit]\nWants=wg-quick@{WG_IFACE}.service\nAfter=wg-quick@{WG_IFACE}.service\n"
            ),
        )?;
        let _ = Command::new("systemctl").arg("daemon-reload").status();
    }
    let unit = format!("wg-quick@{WG_IFACE}");
    let _ = Command::new("systemctl").args(["restart", &unit]).status();
    let ok = Command::new("systemctl")
        .args(["enable", "--now", &unit])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        bail!("No se pudo levantar WireGuard ({unit}). Revisa: journalctl -u {unit}");
    }
    Ok(())
}

fn open_udp(port: u16) {
    if engine::output("ufw", &["status"]).contains("Status: active") {
        let _ = Command::new("ufw")
            .args(["allow", &format!("{port}/udp")])
            .stdout(Stdio::null())
            .status();
        info(format!("ufw: abierto {port}/udp"));
    } else if engine::output("firewall-cmd", &["--state"]) == "running" {
        let _ = Command::new("firewall-cmd")
            .args(["--permanent", &format!("--add-port={port}/udp")])
            .status();
        let _ = Command::new("firewall-cmd").arg("--reload").status();
        info(format!("firewalld: abierto {port}/udp"));
    }
}

// ---------------------------------------------------------------- ha init (principal)

async fn init(
    dir: &Path,
    endpoint: Option<String>,
    wg_port: u16,
    via: Option<String>,
) -> Result<()> {
    let mut env = EnvFile::load(&dir.join(".env"))?;
    if env.get_or("RVMAIL_ROLE", "primary") != "primary" {
        bail!(
            "`ha init` se ejecuta en el servidor principal (este es {}).",
            env.get_or("RVMAIL_ROLE", "")
        );
    }
    let engine = engine::Engine::detect("")?;
    title("Red privada");
    let (bind, wireguard) = match &via {
        Some(ip) => {
            ok(format!(
                "Red privada existente: el respaldo replicará contra {ip}"
            ));
            (ip.clone(), None)
        }
        None => {
            ensure_wireguard()?;
            let old = std::fs::read_to_string(WG_CONF).unwrap_or_default();
            let primary_priv = existing_key(&old, "PrivateKey").map_or_else(wg_genkey, Ok)?;
            let standby_priv = wg_genkey()?;
            let standby_pub = wg_pubkey(&standby_priv)?;
            write_wg(&wg_primary_conf(&primary_priv, wg_port, &standby_pub))?;
            open_udp(wg_port);
            let host = match endpoint {
                Some(e) => e,
                None => detect::public_ip()
                    .await
                    .map(|ip| ip.to_string())
                    .context("No se pudo detectar la IP pública; indícala con --endpoint")?,
            };
            ok(format!(
                "WireGuard {WG_IFACE}: {PRIMARY_WG_IP} (UDP {wg_port})"
            ));
            warn(format!(
                "Abre el puerto UDP {wg_port} en el firewall de tu proveedor (en Oracle: Security List / NSG)."
            ));
            let peer = WgPeer {
                endpoint: format!("{host}:{wg_port}"),
                primary_public_key: wg_pubkey(&primary_priv)?,
                standby_private_key: standby_priv,
            };
            (PRIMARY_WG_IP.to_string(), Some(peer))
        }
    };

    title("Postgres");
    env.set("RVMAIL_HA", "true");
    env.set("COMPOSE_FILE", HA_COMPOSE);
    env.set("PG_REPLICATION_BIND", &bind);
    env.set_default("PG_REPLICATION_PORT", "5433");
    env.set_default("PG_REPLICATION_PASSWORD", &gen_hex(24));
    env.save()?;
    assets::write(dir, false)?;
    engine.compose(dir, &["up", "-d", "postgres"])?;
    wait_pg(&engine, dir, &env)?;

    let pass = env.get_or("PG_REPLICATION_PASSWORD", "");
    psql(
        &engine,
        dir,
        &env,
        &format!(
            "do $$ begin if exists (select from pg_roles where rolname = 'replicator') then \
             alter role replicator with replication login password {p}; \
             else create role replicator with replication login password {p}; end if; end $$",
            p = sql_literal(&pass)
        ),
    )?;
    psql(
        &engine,
        dir,
        &env,
        &format!(
            "select pg_create_physical_replication_slot('{SLOT}') \
             where not exists (select from pg_replication_slots where slot_name = '{SLOT}')"
        ),
    )?;
    let hba = "grep -q '^host replication replicator' \"$PGDATA/pg_hba.conf\" || \
               echo 'host replication replicator all scram-sha-256' >> \"$PGDATA/pg_hba.conf\"";
    let ok_hba = engine
        .exec_cmd(dir, "postgres", &["sh", "-c", hba])
        .status()
        .is_ok_and(|s| s.success());
    if !ok_hba {
        bail!("No se pudo actualizar pg_hba.conf");
    }
    psql(&engine, dir, &env, "select pg_reload_conf()")?;
    ok(format!(
        "Rol replicator, slot {SLOT} y acceso en {bind}:{}",
        env.get_or("PG_REPLICATION_PORT", "5433")
    ));

    // ------------------------------------------------------------ paquete
    let mut standby_env = env.entries();
    standby_env.retain(|(k, _)| k != "PG_REPLICATION_BIND" && k != "RVMAIL_ROLE");
    let bundle = Bundle {
        version: 1,
        replication_host: bind.clone(),
        replication_port: env
            .get_or("PG_REPLICATION_PORT", "5433")
            .parse()
            .unwrap_or(5433),
        replication_password: pass,
        standby_bind: if wireguard.is_some() {
            STANDBY_WG_IP.into()
        } else {
            String::new()
        },
        wireguard,
        env: standby_env,
    };
    let path = dir.join("ha/standby-bundle.json");
    std::fs::create_dir_all(dir.join("ha"))?;
    std::fs::write(&path, serde_json::to_vec_pretty(&bundle)?)?;
    set_private(&path)?;

    title("Siguiente paso, en el servidor de RESPALDO");
    println!("  1. Instala el binario:  curl -fsSL …/get.sh | sudo RVMAIL_NO_INSTALL=1 sh");
    println!("  2. Copia el paquete (contiene secretos; bórralo después):");
    println!("       scp root@<principal>:{} /root/", path.display());
    println!("  3. sudo rvmail ha join --bundle /root/standby-bundle.json");
    warn("El paquete incluye MASTER_KEY y contraseñas: no lo envíes por correo ni chat.");
    Ok(())
}

fn wait_pg(engine: &engine::Engine, dir: &Path, env: &EnvFile) -> Result<()> {
    for _ in 0..40 {
        if psql(engine, dir, env, "select 1").is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    bail!("Postgres no responde")
}

// ---------------------------------------------------------------- ha join (respaldo)

async fn join(dir: &Path, bundle_path: &Path) -> Result<()> {
    let bundle: Bundle =
        serde_json::from_slice(&std::fs::read(bundle_path)?).context("Paquete no válido")?;
    if bundle.version != 1 {
        bail!("Versión de paquete no soportada: {}", bundle.version);
    }
    let engine = engine::Engine::detect("")?;

    title("Configuración del respaldo");
    std::fs::create_dir_all(dir)?;
    assets::write(dir, false)?;
    let env_path = dir.join(".env");
    if env_path.exists() {
        let old = EnvFile::load(&env_path)?;
        if old.get_or("RVMAIL_ROLE", "primary") == "primary" && old.value("MASTER_KEY").is_some() {
            bail!(
                "{} ya tiene una instalación principal. Usa otra carpeta (--dir) o mueve ese .env.",
                dir.display()
            );
        }
    }
    std::fs::write(&env_path, assets::ENV_EXAMPLE)?;
    let mut env = EnvFile::load(&env_path)?;
    for (k, v) in &bundle.env {
        env.set(k, v);
    }
    env.set("RVMAIL_ROLE", "standby");
    env.set("RVMAIL_HA", "true");
    env.set("COMPOSE_FILE", HA_COMPOSE);
    env.set(
        "PG_REPLICATION_BIND",
        if bundle.standby_bind.is_empty() {
            "127.0.0.1"
        } else {
            &bundle.standby_bind
        },
    );
    let profile = detect::profile_for(detect::mem_total_mb());
    for (k, v) in profile.vars {
        env.set(k, v);
    }
    env.save()?;
    ok(format!("{} (perfil {})", env_path.display(), profile.name));

    if let Some(wg) = &bundle.wireguard {
        title("WireGuard");
        ensure_wireguard()?;
        write_wg(&wg_standby_conf(
            &wg.standby_private_key,
            &wg.primary_public_key,
            &wg.endpoint,
        ))?;
        ok(format!(
            "Túnel {WG_IFACE}: {STANDBY_WG_IP} → {PRIMARY_WG_IP} ({})",
            wg.endpoint
        ));
    }

    title("Copia inicial de la base de datos");
    let target = format!("{}:{}", bundle.replication_host, bundle.replication_port);
    let reachable = (0..20).any(|_| {
        let ok = std::net::TcpStream::connect_timeout(
            &target
                .parse()
                .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 5433))),
            std::time::Duration::from_secs(3),
        )
        .is_ok();
        if !ok {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        ok
    });
    if !reachable {
        bail!(
            "No llego a Postgres del principal en {target}. Revisa el túnel (wg show) y el puerto UDP de WireGuard en el firewall."
        );
    }
    let _ = engine.compose(dir, &["stop"]);
    // Crea el volumen con las etiquetas de compose antes de llenarlo
    let _ = engine.compose(dir, &["create", "postgres"]);
    info("pg_basebackup (puede tardar según el tamaño)…");
    basebackup(&engine, dir, &env, &bundle)?;
    engine.compose(dir, &["up", "-d", "postgres"])?;
    wait_pg(&engine, dir, &env)?;
    let recovering = psql(&engine, dir, &env, "select pg_is_in_recovery()")?;
    if recovering != "t" {
        bail!("Postgres arrancó pero no está en modo réplica");
    }
    ok("Réplica en marcha (solo lectura)");
    let _ = std::fs::remove_file(bundle_path);
    info(format!("Paquete borrado: {}", bundle_path.display()));
    println!("\n  Estado: rvmail ha status   ·   Si cae el principal: rvmail failover promote");
    Ok(())
}

fn basebackup(engine: &engine::Engine, dir: &Path, env: &EnvFile, b: &Bundle) -> Result<()> {
    let _ = dir;
    let volume = super::ops::volume_name(engine, env, "postgres_data");
    let script = format!(
        "set -e; D=/var/lib/postgresql/data; mkdir -p $D; find $D -mindepth 1 -delete; chown postgres:postgres $D; chmod 700 $D; \
         su-exec postgres pg_basebackup -h {} -p {} -U replicator -D $D -R -X stream -S {SLOT} -P",
        b.replication_host, b.replication_port
    );
    let status = Command::new(engine.bin())
        .args(["run", "--rm", "--network", "host", "-e"])
        .arg(format!("PGPASSWORD={}", b.replication_password))
        .args(["-v", &format!("{volume}:/var/lib/postgresql/data")])
        .args(["docker.io/library/postgres:16-alpine", "sh", "-c", &script])
        .status()
        .context("No se pudo lanzar pg_basebackup")?;
    if !status.success() {
        bail!("pg_basebackup falló (¿contraseña de replicación o pg_hba del principal?)");
    }
    Ok(())
}

// ---------------------------------------------------------------- estado

/// Segundos desde el último contacto del túnel (None si no hay túnel de RV Mail).
pub fn wg_handshake_age() -> Option<u64> {
    if !Path::new(WG_CONF).exists() {
        return None;
    }
    let out = engine::output("wg", &["show", WG_IFACE, "latest-handshakes"]);
    let ts: u64 = out.split_whitespace().nth(1)?.parse().ok()?;
    if ts == 0 {
        return Some(u64::MAX);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(now.saturating_sub(ts))
}

pub struct ReplicaState {
    pub role: String,
    pub detail: String,
    pub healthy: bool,
}

pub fn status(dir: &Path) -> Result<ReplicaState> {
    let env = EnvFile::load(&dir.join(".env"))?;
    let engine = engine::Engine::detect("")?;
    let role = env.get_or("RVMAIL_ROLE", "primary");
    let st = if role == "standby" {
        let row = psql(
            &engine,
            dir,
            &env,
            "select pg_is_in_recovery(), coalesce((select status from pg_stat_wal_receiver), 'sin conexión'), \
             coalesce(extract(epoch from now() - pg_last_xact_replay_timestamp())::int, -1)",
        )?;
        let cols: Vec<&str> = row.split('|').collect();
        let lag: i64 = cols.get(2).and_then(|v| v.parse().ok()).unwrap_or(-1);
        let streaming = cols.get(1) == Some(&"streaming");
        ReplicaState {
            role,
            healthy: cols.first() == Some(&"t") && streaming,
            detail: format!(
                "réplica {} · último cambio aplicado hace {}",
                cols.get(1).unwrap_or(&"?"),
                if lag < 0 {
                    "—".into()
                } else {
                    format!("{lag} s")
                }
            ),
        }
    } else if env.get_or("RVMAIL_HA", "false") == "true" {
        let row = psql(
            &engine,
            dir,
            &env,
            &format!(
                "select s.active, coalesce(pg_size_pretty(pg_wal_lsn_diff(pg_current_wal_lsn(), s.restart_lsn)), '0'), \
                 coalesce((select client_addr::text || ' ' || state from pg_stat_replication limit 1), '') \
                 from pg_replication_slots s where s.slot_name = '{SLOT}'"
            ),
        )?;
        let cols: Vec<&str> = row.split('|').collect();
        ReplicaState {
            role,
            healthy: cols.first() == Some(&"t"),
            detail: if row.is_empty() {
                "no existe el slot de replicación (ejecuta rvmail ha init)".into()
            } else {
                format!(
                    "respaldo {} · WAL pendiente {}",
                    if cols.first() == Some(&"t") {
                        "conectado"
                    } else {
                        "DESCONECTADO"
                    },
                    cols.get(1).unwrap_or(&"?")
                )
            },
        }
    } else {
        ReplicaState {
            role,
            healthy: true,
            detail: "sin respaldo configurado (rvmail ha init)".into(),
        }
    };
    Ok(st)
}

// ---------------------------------------------------------------- promoción

async fn promote(dir: &Path, ui: super::Ui, yes: bool, restore_mail: bool) -> Result<()> {
    let mut env = EnvFile::load(&dir.join(".env"))?;
    if env.get_or("RVMAIL_ROLE", "") != "standby" {
        bail!(
            "Este servidor no es un respaldo (RVMAIL_ROLE={}).",
            env.get_or("RVMAIL_ROLE", "primary")
        );
    }
    warn("Promover convierte este respaldo en el servidor PRINCIPAL.");
    warn("Asegúrate de que el principal anterior está apagado o aislado (evita dos principales).");
    if !yes && !ui.confirm("¿Promover ahora?", false)? {
        bail!("Cancelado");
    }
    let engine = engine::Engine::detect("")?;
    let promoted = psql(&engine, dir, &env, "select pg_promote(true, 60)")?;
    if promoted != "t" {
        bail!("Postgres no confirmó la promoción");
    }
    ok("Postgres ahora acepta escrituras");
    env.set("RVMAIL_ROLE", "primary");
    env.save()?;

    if restore_mail && env.get_or("MAILSERVER_PROVIDER", "") == "stalwart" {
        super::ops::restore_stalwart_from_restic(&engine, dir, &env)?;
    }
    engine.compose(dir, &["up", "-d", "--remove-orphans"])?;
    if engine::wait_healthy(&env.get_or("HTTP_PORT", "8088"), 180).await {
        ok("RV Mail funcionando en este servidor");
    } else {
        warn("La app aún no responde: rvmail doctor");
    }
    title("Ahora");
    println!(
        "  1. Apunta el DNS (A) de {} a este servidor",
        env.get_or("DOMAIN", "tu dominio")
    );
    if let Some(h) = env.value("MAILSERVER_HOSTNAME") {
        println!("     y también {h} (MX).");
    }
    println!("  2. Publica la app en el proxy de este servidor (o usa el modo caddy).");
    println!("  3. Cuando el antiguo principal vuelva, reinstálalo como respaldo:");
    println!("     aquí `rvmail ha init`, allí `rvmail ha join --bundle …` (en otra carpeta o tras mover su .env).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wireguard_configs() {
        let p = wg_primary_conf("PRIV", 51820, "SPUB");
        assert!(p.contains("Address = 10.57.0.1/24") && p.contains("ListenPort = 51820"));
        assert!(p.contains("PublicKey = SPUB") && p.contains("AllowedIPs = 10.57.0.2/32"));
        assert_eq!(existing_key(&p, "PrivateKey").as_deref(), Some("PRIV"));
        let s = wg_standby_conf("SPRIV", "PPUB", "1.2.3.4:51820");
        assert!(s.contains("Endpoint = 1.2.3.4:51820") && s.contains("PersistentKeepalive = 25"));
    }

    #[test]
    fn escapes_sql() {
        assert_eq!(sql_literal("a'b"), "'a''b'");
    }
}
