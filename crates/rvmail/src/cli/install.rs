//! `rvmail install`: asistente de instalación con autodetección.

use super::envfile::{gen_hex, gen_master_key, EnvFile};
use super::{assets, config, detect, engine, info, ok, stalwart_setup, title, warn, Common, Ui};
use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::Path;

#[derive(Args)]
pub struct InstallArgs {
    #[command(flatten)]
    pub common: Common,
    /// Dominio público de la app (ej. correo.solucionesrv.net)
    #[arg(long)]
    pub domain: Option<String>,
    /// caddy (TLS automático en 80/443) | external (aaPanel/nginx ya usa 80/443) | auto
    #[arg(long, default_value = "auto")]
    pub mode: String,
    /// Correo para Let's Encrypt (modo caddy)
    #[arg(long)]
    pub email: Option<String>,
    /// Puerto local en modo external
    #[arg(long)]
    pub port: Option<u16>,
    /// aapanel | stalwart | none | auto
    #[arg(long, default_value = "auto")]
    pub mailserver: String,
    /// Nombre del servidor de correo (ej. mail.solucionesrv.net)
    #[arg(long)]
    pub mail_host: Option<String>,
    /// Dominio de correo principal (Stalwart; por defecto el del servidor de correo)
    #[arg(long)]
    pub mail_domain: Option<String>,
    /// Clave de la API de aaPanel
    #[arg(long, env = "AAPANEL_API_KEY", hide_env_values = true)]
    pub aapanel_key: Option<String>,
    /// docker | podman | quadlet | auto
    #[arg(long, default_value = "auto")]
    pub engine: String,
    /// primary (servidor principal) | standby (respaldo)
    #[arg(long, default_value = "primary")]
    pub role: String,
    /// Prefijo de las imágenes (ghcr.io/… = precompiladas; "rvmail" = compilar en el VPS)
    #[arg(long)]
    pub image: Option<String>,
    /// Versión de las imágenes (APP_VERSION): latest, 0.3.0…
    #[arg(long)]
    pub tag: Option<String>,
    /// Perfil de recursos: auto | small | standard | large
    #[arg(long, default_value = "auto")]
    pub profile: String,
    /// Solo escribe la configuración, sin arrancar servicios
    #[arg(long)]
    pub no_start: bool,
}

pub async fn run(a: InstallArgs) -> Result<()> {
    let ui = a.common.ui();
    let dir = a.common.dir();
    if !super::is_root() {
        warn("No eres root: si falla algún paso, repite con sudo.");
    }

    title("Detectando el servidor");
    let mem = detect::mem_total_mb();
    ok(format!(
        "Arquitectura {} · {} CPU · {} MB de RAM",
        detect::arch(),
        detect::cpus(),
        mem
    ));
    let engine = ensure_engine(&a.engine, ui)?;
    ok(format!("Contenedores: {}", engine.label()));
    let panel = detect::aapanel();
    if panel.installed {
        ok(format!(
            "aaPanel (puerto {}){}",
            panel.port,
            if panel.mail_server {
                " con Mail Server"
            } else {
                ""
            }
        ));
    }
    let public_ip = detect::public_ip().await;
    match public_ip {
        Some(ip) => ok(format!("IP pública: {ip}")),
        None => warn("No se pudo averiguar la IP pública (sin salida a internet?)"),
    }

    // ------------------------------------------------------------ .env base
    std::fs::create_dir_all(&dir).with_context(|| format!("No se pudo crear {}", dir.display()))?;
    let written = assets::write(&dir, false)?;
    if !written.is_empty() {
        info(format!(
            "Archivos de despliegue en {}: {}",
            dir.display(),
            written.join(", ")
        ));
    }
    let env_path = dir.join(".env");
    let fresh = !env_path.exists();
    if fresh {
        std::fs::write(&env_path, assets::ENV_EXAMPLE)?;
    }
    let mut env = EnvFile::load(&env_path)?;
    for k in ["POSTGRES_PASSWORD", "REDIS_PASSWORD"] {
        env.set_default(k, &gen_hex(24));
    }
    env.set_default("JWT_SECRET", &gen_hex(32));
    env.set_default("MASTER_KEY", &gen_master_key());
    if fresh {
        ok("Archivo .env creado con secretos nuevos");
    } else {
        info("Usando el .env existente (los secretos no cambian)");
    }

    // ------------------------------------------------------------ dominio
    title("Dominio");
    let current = env.value("DOMAIN").filter(|d| d != "correo.example.com");
    let mut domain = a.domain.clone().or(current).unwrap_or_default();
    loop {
        if domain.is_empty() {
            if !ui.interactive {
                bail!("Falta --domain");
            }
            domain = ui.input("Dominio para RV Mail (ej. correo.solucionesrv.net)", "")?;
        }
        if detect::valid_domain(&domain) {
            break;
        }
        if !ui.interactive {
            bail!("Dominio no válido: {domain}");
        }
        warn(format!("Dominio no válido: {domain}"));
        domain.clear();
    }
    domain = domain.to_lowercase();
    env.set("DOMAIN", &domain);
    check_dns(&domain, public_ip).await;

    // ------------------------------------------------------------ proxy
    let busy_web = detect::port_busy(80) || detect::port_busy(443);
    let mode = match a.mode.as_str() {
        "auto" => {
            let default = usize::from(busy_web);
            let items = vec![
                "caddy — RV Mail gestiona HTTPS en 80/443 (VPS limpio)".to_string(),
                "external — ya hay aaPanel/nginx en 80/443; la app escucha en 127.0.0.1"
                    .to_string(),
            ];
            if busy_web {
                info("Los puertos 80/443 ya están en uso: se recomienda external");
            }
            if ui.select("Modo de publicación", &items, default)? == 0 {
                "caddy"
            } else {
                "external"
            }
        }
        "caddy" => "caddy",
        "external" => "external",
        other => bail!("--mode debe ser caddy, external o auto (no {other})"),
    };
    if mode == "caddy" && busy_web && !engine.quadlet {
        warn("80/443 están ocupados: Caddy no podrá arrancar hasta liberarlos.");
    }
    env.set("PROXY_MODE", mode);
    let port = match a.port {
        Some(p) => p,
        None => {
            let wanted: u16 = env.get_or("HTTP_PORT", "8088").parse().unwrap_or(8088);
            (wanted..wanted + 50)
                .find(|p| !detect::port_busy(*p) || fresh_is_ours(&env, *p))
                .unwrap_or(wanted)
        }
    };
    env.set("HTTP_PORT", &port.to_string());
    if mode == "caddy" {
        let email = a
            .email
            .clone()
            .or(env.value("ACME_EMAIL"))
            .unwrap_or_default();
        let email = ui.input("Correo para Let's Encrypt (opcional)", &email)?;
        env.set("ACME_EMAIL", &email);
    }

    // ------------------------------------------------------------ servidor de correo
    title("Servidor de correo");
    let choice = match a.mailserver.as_str() {
        "auto" => {
            let current = env.get_or("MAILSERVER_PROVIDER", "");
            let default = if panel.mail_server {
                "aapanel"
            } else if !current.is_empty() && current != "none" {
                current.as_str()
            } else {
                "none"
            };
            config::choose_mailserver(ui, &panel, default)?
        }
        m @ ("aapanel" | "stalwart" | "none") => m.to_string(),
        other => bail!("--mailserver debe ser aapanel, stalwart, none o auto (no {other})"),
    };
    let opts = config::MailOpts {
        provider: choice.clone(),
        mail_host: a.mail_host.clone(),
        mail_domain: a.mail_domain.clone(),
        aapanel_key: a.aapanel_key.clone(),
    };
    config::apply_mailserver(ui, &mut env, &panel, &domain, opts)?;

    // ------------------------------------------------------------ SSO
    title("Inicio de sesión con Google, Microsoft y Yahoo");
    if ui.confirm(
        "¿Configurar ahora el inicio de sesión con Google/Microsoft/Yahoo?",
        false,
    )? {
        config::sso_menu(ui, &mut env)?;
    } else {
        info("Puedes hacerlo después con: rvmail config sso");
    }

    // ------------------------------------------------------------ rol, recursos e imágenes
    title("Recursos");
    let role = match a.role.as_str() {
        r @ ("primary" | "standby") => r,
        other => bail!("--role debe ser primary o standby (no {other})"),
    };
    env.set("RVMAIL_ROLE", role);
    let profile = match a.profile.as_str() {
        "small" => detect::profile_for(1024),
        "standard" => detect::profile_for(4096),
        "large" => detect::profile_for(8192),
        _ => detect::profile_for(mem),
    };
    for (k, v) in profile.vars {
        env.set(k, v);
    }
    ok(format!("Perfil de recursos: {}", profile.name));
    let in_repo = dir.join("Dockerfile").exists() && dir.join("crates").exists();
    let image = a
        .image
        .clone()
        .unwrap_or_else(|| match env.value("IMAGE_PREFIX") {
            Some(p) if p != "rvmail" || in_repo => p,
            _ if in_repo => "rvmail".into(),
            _ => super::DEFAULT_IMAGE.into(),
        });
    if image == "rvmail" && (!in_repo || engine.quadlet) {
        bail!("IMAGE_PREFIX=rvmail compila en el VPS y requiere el repositorio y Docker/podman compose.");
    }
    env.set("IMAGE_PREFIX", &image);
    if let Some(tag) = &a.tag {
        env.set("APP_VERSION", tag);
    }
    let mut profiles = vec![];
    if mode == "caddy" {
        profiles.push("caddy");
    }
    if choice == "stalwart" {
        profiles.push("stalwart");
    }
    env.set("COMPOSE_PROFILES", &profiles.join(","));
    env.save()?;
    ok(format!(
        "Configuración guardada en {}",
        env.path().display()
    ));

    if a.no_start {
        info("Sin arrancar (--no-start). Para iniciar: rvmail install --dir … o docker compose up -d");
        return Ok(());
    }

    // ------------------------------------------------------------ arranque
    title("Arrancando servicios");
    start(&engine, &dir, &image).await?;

    if choice == "stalwart" {
        let mail_host = env.get_or("MAILSERVER_HOSTNAME", "");
        let mail_domain = env.get_or("RVMAIL_MAIL_DOMAIN", detect::parent_domain(&mail_host));
        info("Configurando Stalwart…");
        match stalwart_setup::ensure(&mut env, &mail_host, &mail_domain).await? {
            stalwart_setup::Outcome::AlreadyDone => ok("Stalwart ya estaba configurado"),
            stalwart_setup::Outcome::Bootstrapped { user } => {
                ok(format!(
                    "Stalwart configurado (administrador {user}, clave en .env)"
                ));
                engine.compose(&dir, &["restart", "stalwart"])?;
                engine.compose(&dir, &["up", "-d", "--remove-orphans"])?;
            }
        }
    }

    summary(&env, mode, &choice, role);
    Ok(())
}

/// Si el puerto lo usa nuestra propia instalación (reinstalación), no cuenta como ocupado.
fn fresh_is_ours(env: &EnvFile, port: u16) -> bool {
    env.value("HTTP_PORT")
        .is_some_and(|p| p == port.to_string())
}

fn ensure_engine(preferred: &str, ui: Ui) -> Result<engine::Engine> {
    let pref = if preferred == "auto" { "" } else { preferred };
    if let Ok(e) = engine::Engine::detect(pref) {
        return Ok(e);
    }
    if !pref.is_empty() && pref != "docker" {
        return engine::Engine::detect(pref);
    }
    warn("No hay Docker ni Podman instalados.");
    if !ui.confirm(
        "¿Instalar Docker ahora (script oficial get.docker.com)?",
        true,
    )? {
        bail!("Hace falta Docker o Podman.");
    }
    let ok = std::process::Command::new("sh")
        .args([
            "-c",
            "curl -fsSL https://get.docker.com | sh && systemctl enable --now docker",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        bail!("No se pudo instalar Docker.");
    }
    engine::Engine::detect("docker")
}

async fn check_dns(domain: &str, public_ip: Option<std::net::IpAddr>) {
    let ips = detect::resolve(domain).await;
    match (ips.is_empty(), public_ip) {
        (true, _) => warn(format!("{domain} todavía no resuelve: crea un registro A hacia la IP del VPS")),
        (false, Some(ip)) if ips.contains(&ip) => ok(format!("{domain} apunta a este servidor ({ip})")),
        (false, Some(ip)) => warn(format!(
            "{domain} apunta a {} y este servidor es {ip} (¿Cloudflare con proxy o DNS sin actualizar?)",
            ips.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )),
        (false, None) => ok(format!("{domain} resuelve")),
    }
}

pub async fn start(engine: &engine::Engine, dir: &Path, image: &str) -> Result<()> {
    if image == "rvmail" {
        info("Compilando la imagen en el VPS (la primera vez tarda varios minutos)…");
        engine.compose(dir, &["build", "--pull"])?;
    } else {
        info("Descargando imágenes…");
        engine.compose(dir, &["pull"])?;
    }
    engine.compose(dir, &["up", "-d", "--remove-orphans"])?;
    let env = EnvFile::load(&dir.join(".env"))?;
    info("Esperando a que la aplicación responda…");
    if !engine::wait_healthy(&env.get_or("HTTP_PORT", "8088"), 180).await {
        let _ = engine.compose(dir, &["ps"]);
        bail!("La app no respondió. Revisa los registros: rvmail doctor  ·  docker compose logs app worker");
    }
    ok("RV Mail está funcionando");
    Ok(())
}

fn summary(env: &EnvFile, mode: &str, mailserver: &str, role: &str) {
    let domain = env.get_or("DOMAIN", "");
    title("Listo");
    if mode == "caddy" {
        println!("  URL: https://{domain}");
        println!("  Caddy pedirá el certificado en cuanto el DNS apunte a este servidor.");
    } else {
        println!(
            "  La app escucha en 127.0.0.1:{}",
            env.get_or("HTTP_PORT", "8088")
        );
        println!("  Publícala en tu proxy (aaPanel → Website → {domain}, SSL de Let's Encrypt)");
        println!("  con la configuración de deploy/nginx-aapanel.conf.example.");
    }
    match mailserver {
        "aapanel" => {
            println!(
                "\n  Mail Server de aaPanel: en aaPanel → Settings → API interface, añade a la"
            );
            println!(
                "  lista blanca la IP {}",
                env.get_or("API_IP", "172.30.57.10")
            );
            if env.value("AAPANEL_API_KEY").is_none() {
                println!("  y guarda la clave con: rvmail config mailserver");
            }
        }
        "stalwart" => {
            let host = env.get_or("MAILSERVER_HOSTNAME", "");
            println!("\n  Stalwart ({host}): publica https://{host} en tu proxy hacia");
            println!(
                "  127.0.0.1:{} para JMAP. Dominios y DNS: Configuración → Servidor de correo.",
                env.get_or("STALWART_HTTP_PORT", "8180")
            );
        }
        _ => {}
    }
    if role == "standby" {
        println!("\n  Rol: respaldo. La réplica desde el principal se configura en la fase H (docs/HA.md).");
    }
    println!("\n  Primer acceso: crea la cuenta de administrador en la pantalla inicial.");
    println!("  Diagnóstico: rvmail doctor   ·   Cambiar ajustes: rvmail config");
    warn("RESPALDA el .env (sobre todo MASTER_KEY) fuera del servidor.");
}
