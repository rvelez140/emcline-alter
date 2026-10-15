//! `rvmail config`: ver y cambiar la configuración después de instalar.
//! Reescribe `.env` y aplica los cambios (compose recrea solo lo que cambió).

use super::envfile::{gen_hex, mask, EnvFile};
use super::{assets, detect, engine, info, ok, stalwart_setup, title, warn, Common, Ui};
use anyhow::{bail, Result};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct ConfigArgs {
    #[command(flatten)]
    pub common: Common,
    #[command(subcommand)]
    pub what: Option<ConfigCmd>,
    /// Guardar sin reiniciar servicios
    #[arg(long, global = true)]
    pub no_apply: bool,
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Muestra la configuración (los secretos, ocultos)
    Show,
    /// Muestra una variable
    Get { key: String },
    /// Cambia variables: rvmail config set CLAVE=valor [CLAVE=valor…]
    Set { pairs: Vec<String> },
    /// Dominio y modo de publicación (caddy/external)
    Domain,
    /// Servidor de correo integrado (aaPanel, Stalwart o ninguno)
    Mailserver,
    /// Inicio de sesión con Google, Microsoft y Yahoo
    Sso,
    /// Perfil de recursos (memoria) y límites del worker
    Resources,
}

pub async fn run(a: ConfigArgs) -> Result<()> {
    let ui = a.common.ui();
    let dir = a.common.dir();
    let path = dir.join(".env");
    if !path.exists() {
        bail!(
            "No hay instalación en {} (falta .env). Usa: rvmail install",
            dir.display()
        );
    }
    let mut env = EnvFile::load(&path)?;
    let before = env.entries();

    let what = match a.what {
        Some(w) => w,
        None if ui.interactive => menu(ui)?,
        None => ConfigCmd::Show,
    };
    match what {
        ConfigCmd::Show => {
            show(&env);
            return Ok(());
        }
        ConfigCmd::Get { key } => {
            match env.get(&key) {
                Some(v) => println!("{v}"),
                None => bail!("{key} no está definida"),
            }
            return Ok(());
        }
        ConfigCmd::Set { pairs } => {
            let known: Vec<String> = EnvFile::parse(&path, assets::ENV_EXAMPLE)
                .entries()
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            for p in pairs {
                let Some((k, v)) = p.split_once('=') else {
                    bail!("Formato: CLAVE=valor ({p})")
                };
                let k = k.trim();
                if !k
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    || k.is_empty()
                {
                    bail!("Nombre de variable no válido: {k}");
                }
                if !known.iter().any(|x| x == k) && !k.starts_with("RVMAIL_") {
                    warn(format!(
                        "{k} no es una variable conocida de RV Mail (se guarda igual)"
                    ));
                }
                env.set(k, v);
            }
        }
        ConfigCmd::Domain => domain(ui, &mut env)?,
        ConfigCmd::Mailserver => {
            let panel = detect::aapanel();
            let current = env.get_or("MAILSERVER_PROVIDER", "none");
            let choice = choose_mailserver(ui, &panel, &current)?;
            let domain = env.get_or("DOMAIN", "");
            apply_mailserver(
                ui,
                &mut env,
                &panel,
                &domain,
                MailOpts {
                    provider: choice,
                    ..Default::default()
                },
            )?;
            set_profiles(&mut env);
        }
        ConfigCmd::Sso => sso_menu(ui, &mut env)?,
        ConfigCmd::Resources => resources(ui, &mut env)?,
    }

    let after = env.entries();
    let changed: Vec<String> = after
        .iter()
        .filter(|kv| !before.contains(kv))
        .map(|(k, _)| k.clone())
        .collect();
    if changed.is_empty() {
        info("Sin cambios");
        return Ok(());
    }
    env.save()?;
    ok(format!("Guardado: {}", changed.join(", ")));
    if a.no_apply {
        info("Sin aplicar (--no-apply). Para aplicar: rvmail config --dir … y elegir aplicar, o docker compose up -d");
        return Ok(());
    }
    apply(&dir, &mut env).await
}

/// Aplica la configuración: compose recrea solo los contenedores cuyo entorno cambió.
pub async fn apply(dir: &std::path::Path, env: &mut EnvFile) -> Result<()> {
    let engine = engine::Engine::detect("")?;
    info("Aplicando cambios…");
    engine.compose(dir, &["up", "-d", "--remove-orphans"])?;
    if env.get_or("MAILSERVER_PROVIDER", "") == "stalwart"
        && !stalwart_setup::admin_works(env).await
    {
        let host = env.get_or("MAILSERVER_HOSTNAME", "");
        let domain = env.get_or("RVMAIL_MAIL_DOMAIN", detect::parent_domain(&host));
        if let stalwart_setup::Outcome::Bootstrapped { user } =
            stalwart_setup::ensure(env, &host, &domain).await?
        {
            ok(format!("Stalwart configurado (administrador {user})"));
            engine.compose(dir, &["restart", "stalwart"])?;
            engine.compose(dir, &["up", "-d", "--remove-orphans"])?;
        }
    }
    if engine::wait_healthy(&env.get_or("HTTP_PORT", "8088"), 120).await {
        ok("Cambios aplicados; RV Mail responde");
        Ok(())
    } else {
        bail!("La app no responde tras aplicar los cambios. Revisa: rvmail doctor")
    }
}

fn menu(ui: Ui) -> Result<ConfigCmd> {
    let items: Vec<String> = [
        "Ver la configuración",
        "Dominio y publicación (caddy/external)",
        "Servidor de correo (aaPanel / Stalwart / ninguno)",
        "Inicio de sesión con Google, Microsoft y Yahoo",
        "Recursos (memoria, cuentas por worker)",
        "Cambiar una variable",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    Ok(match ui.select("¿Qué quieres cambiar?", &items, 0)? {
        0 => ConfigCmd::Show,
        1 => ConfigCmd::Domain,
        2 => ConfigCmd::Mailserver,
        3 => ConfigCmd::Sso,
        4 => ConfigCmd::Resources,
        _ => {
            let k = ui.input("Variable", "")?;
            let v = ui.input("Valor", "")?;
            ConfigCmd::Set {
                pairs: vec![format!("{k}={v}")],
            }
        }
    })
}

fn show(env: &EnvFile) {
    for (k, v) in env.entries() {
        println!("{k}={}", mask(&k, &v));
    }
}

fn set_profiles(env: &mut EnvFile) {
    let mut p = vec![];
    if env.get_or("PROXY_MODE", "") == "caddy" {
        p.push("caddy");
    }
    if env.get_or("MAILSERVER_PROVIDER", "") == "stalwart" {
        p.push("stalwart");
    }
    env.set("COMPOSE_PROFILES", &p.join(","));
}

fn domain(ui: Ui, env: &mut EnvFile) -> Result<()> {
    let d = ui
        .input("Dominio de RV Mail", &env.get_or("DOMAIN", ""))?
        .to_lowercase();
    if !detect::valid_domain(&d) {
        bail!("Dominio no válido: {d}");
    }
    env.set("DOMAIN", &d);
    let current = usize::from(env.get_or("PROXY_MODE", "external") == "external");
    let items = vec![
        "caddy (HTTPS automático en 80/443)".to_string(),
        "external (proxy existente)".to_string(),
    ];
    let mode = if ui.select("Modo de publicación", &items, current)? == 0 {
        "caddy"
    } else {
        "external"
    };
    env.set("PROXY_MODE", mode);
    if mode == "caddy" {
        let e = ui.input(
            "Correo para Let's Encrypt (opcional)",
            &env.get_or("ACME_EMAIL", ""),
        )?;
        env.set("ACME_EMAIL", &e);
    } else {
        let p = ui.input("Puerto local", &env.get_or("HTTP_PORT", "8088"))?;
        if p.parse::<u16>().is_err() {
            bail!("Puerto no válido: {p}");
        }
        env.set("HTTP_PORT", &p);
    }
    set_profiles(env);
    Ok(())
}

fn resources(ui: Ui, env: &mut EnvFile) -> Result<()> {
    let mem = detect::mem_total_mb();
    let items: Vec<String> = [1024, 4096, 8192]
        .iter()
        .map(|m| detect::profile_for(*m).name.to_string())
        .collect();
    let rec = match mem {
        m if m > 0 && m < 1536 => 0,
        m if m >= 6144 => 2,
        _ => 1,
    };
    info(format!("Este servidor tiene {mem} MB de RAM"));
    let i = ui.select("Perfil de recursos", &items, rec)?;
    for (k, v) in detect::profile_for([1024, 4096, 8192][i]).vars {
        env.set(k, v);
    }
    let days = ui.input(
        "Días de correo a sincronizar por carpeta",
        &env.get_or("SYNC_DAYS", "90"),
    )?;
    env.set("SYNC_DAYS", &days);
    Ok(())
}

// ---------------------------------------------------------------- servidor de correo

#[derive(Default)]
pub struct MailOpts {
    pub provider: String,
    pub mail_host: Option<String>,
    pub mail_domain: Option<String>,
    pub aapanel_key: Option<String>,
}

pub fn choose_mailserver(ui: Ui, panel: &detect::AaPanel, default: &str) -> Result<String> {
    let busy25 = detect::port_busy(25);
    let mut keys = vec![];
    let mut items = vec![];
    if panel.installed {
        keys.push("aapanel");
        items.push(format!(
            "aaPanel — administrar el Mail Server de aaPanel{}",
            if panel.mail_server {
                " (detectado)"
            } else {
                " (no detectado el plugin mail_sys)"
            }
        ));
    }
    keys.push("stalwart");
    items.push(format!(
        "Stalwart — servidor de correo en Rust con JMAP, incluido{}",
        if busy25 {
            " (¡el puerto 25 ya está en uso!)"
        } else {
            ""
        }
    ));
    keys.push("none");
    items.push("Ninguno — solo cliente (Gmail, Outlook, cPanel…)".into());
    let def = keys
        .iter()
        .position(|k| *k == default)
        .unwrap_or(keys.len() - 1);
    Ok(keys[ui.select("Servidor de correo integrado", &items, def)?].to_string())
}

pub fn apply_mailserver(
    ui: Ui,
    env: &mut EnvFile,
    panel: &detect::AaPanel,
    app_domain: &str,
    o: MailOpts,
) -> Result<()> {
    let base = detect::parent_domain(app_domain).to_string();
    env.set("MAILSERVER_PROVIDER", &o.provider);
    match o.provider.as_str() {
        "aapanel" => {
            let guess = o
                .mail_host
                .or(env.value("MAILSERVER_HOSTNAME"))
                .or_else(detect::postfix_hostname)
                .unwrap_or_else(|| format!("mail.{base}"));
            let host = ui.input(
                "Nombre del servidor de correo (el del certificado TLS)",
                &guess,
            )?;
            env.set("MAILSERVER_HOSTNAME", &host);
            env.set(
                "AAPANEL_URL",
                &format!(
                    "https://host.docker.internal:{}",
                    if panel.port > 0 { panel.port } else { 7800 }
                ),
            );
            let key = match o.aapanel_key {
                Some(k) => k,
                None => {
                    info("aaPanel → Settings → API interface: actívala y copia la clave.");
                    ui.password("Clave de la API de aaPanel (Enter para dejarla como está)")?
                }
            };
            if !key.is_empty() {
                env.set("AAPANEL_API_KEY", &key);
            }
            ok(format!("Mail Server de aaPanel: {host}"));
        }
        "stalwart" => {
            for p in [25u16, 465, 587, 993] {
                if detect::port_busy(p) && !is_stalwart_running() {
                    warn(format!("El puerto {p} está en uso (¿Postfix/Dovecot de aaPanel?). Stalwart no podrá usarlo."));
                }
            }
            let guess = o
                .mail_host
                .or(env.value("MAILSERVER_HOSTNAME"))
                .unwrap_or_else(|| format!("mail.{base}"));
            let host = ui.input(
                "Nombre del servidor de correo (ej. mail.tudominio.com)",
                &guess,
            )?;
            let dguess = o
                .mail_domain
                .or(env.value("RVMAIL_MAIL_DOMAIN"))
                .unwrap_or_else(|| detect::parent_domain(&host).to_string());
            let mail_domain = ui.input("Dominio de correo principal", &dguess)?;
            env.set("MAILSERVER_HOSTNAME", &host);
            env.set("RVMAIL_MAIL_DOMAIN", &mail_domain);
            env.set_default("STALWART_URL", "http://stalwart:8080");
            env.set_default("STALWART_RECOVERY_PASSWORD", &gen_hex(16));
            env.set_default("MAILSERVER_JMAP_URL", &format!("https://{host}"));
            ok(format!("Stalwart: {host} (correo @{mail_domain})"));
        }
        _ => info("Sin servidor de correo integrado: RV Mail funciona como cliente."),
    }
    set_profiles(env);
    Ok(())
}

fn is_stalwart_running() -> bool {
    ["docker", "podman"]
        .iter()
        .any(|bin| engine::output(bin, &["ps", "--format", "{{.Image}}"]).contains("stalwart"))
}

// ---------------------------------------------------------------- SSO (fase S3b)

struct SsoProvider {
    key: &'static str,
    name: &'static str,
    console: &'static str,
}

const SSO: [SsoProvider; 3] = [
    SsoProvider {
        key: "GOOGLE",
        name: "Google",
        console: "https://console.cloud.google.com/apis/credentials",
    },
    SsoProvider {
        key: "MICROSOFT",
        name: "Microsoft",
        console: "https://entra.microsoft.com → Aplicaciones → Registros de aplicaciones",
    },
    SsoProvider {
        key: "YAHOO",
        name: "Yahoo",
        console: "https://developer.yahoo.com/apps/",
    },
];

pub fn sso_menu(ui: Ui, env: &mut EnvFile) -> Result<()> {
    let domain = env.get_or("DOMAIN", "tu-dominio");
    loop {
        let mut items: Vec<String> = SSO
            .iter()
            .map(|p| {
                let on = env.value(&format!("SSO_{}_CLIENT_ID", p.key)).is_some();
                format!(
                    "{} — {}",
                    p.name,
                    if on { "configurado" } else { "sin configurar" }
                )
            })
            .collect();
        let auto = env.get_or("SSO_AUTO_SIGNUP", "false") == "true";
        items.push(format!(
            "Alta automática de usuarios nuevos: {}",
            if auto { "sí" } else { "no" }
        ));
        items.push("Terminar".into());
        if !ui.interactive {
            return Ok(());
        }
        let i = ui.select("Inicio de sesión con proveedores", &items, items.len() - 1)?;
        if i == items.len() - 1 {
            return Ok(());
        }
        if i == SSO.len() {
            let v = ui.confirm(
                "¿Crear la cuenta de RV Mail la primera vez que alguien entra con SSO?",
                auto,
            )?;
            env.set("SSO_AUTO_SIGNUP", if v { "true" } else { "false" });
            continue;
        }
        let p = &SSO[i];
        title(format!("{} (guía completa: docs/SSO.md)", p.name));
        println!("  1. Crea una app en {}", p.console);
        println!(
            "  2. URL de retorno: https://{domain}/api/auth/sso/{}/callback",
            p.key.to_lowercase()
        );
        let id_key = format!("SSO_{}_CLIENT_ID", p.key);
        let secret_key = format!("SSO_{}_CLIENT_SECRET", p.key);
        let id = ui.input("Client ID (vacío = desactivar)", &env.get_or(&id_key, ""))?;
        if id.is_empty() {
            env.set(&id_key, "");
            env.set(&secret_key, "");
            info(format!("{} desactivado", p.name));
            continue;
        }
        env.set(&id_key, &id);
        let secret = ui.password("Client secret (Enter para dejarlo como está)")?;
        if !secret.is_empty() {
            env.set(&secret_key, &secret);
        }
        if p.key == "MICROSOFT" {
            let t = ui.input(
                "Inquilino (common, organizations, consumers o el ID de tu Entra ID)",
                &env.get_or("SSO_MICROSOFT_TENANT", "common"),
            )?;
            env.set("SSO_MICROSOFT_TENANT", &t);
        }
        if env.value(&secret_key).is_none() {
            warn(format!(
                "Falta el client secret de {}: el botón no aparecerá hasta ponerlo",
                p.name
            ));
        } else {
            ok(format!("{} configurado", p.name));
        }
    }
}
