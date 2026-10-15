//! `rvmail doctor`: revisa la instalación de punta a punta y dice qué arreglar.

use super::envfile::EnvFile;
use super::{detect, engine, fail, ok, stalwart_setup, title, warn, Common};
use anyhow::Result;
use clap::Args;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

#[derive(Args)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub common: Common,
    /// No hacer comprobaciones hacia internet (DNS público, HTTPS, salida por el 25)
    #[arg(long)]
    pub offline: bool,
}

#[derive(Default)]
struct Report {
    warnings: usize,
    failures: usize,
}

impl Report {
    fn ok(&mut self, m: impl AsRef<str>) {
        ok(m);
    }
    fn warn(&mut self, m: impl AsRef<str>) {
        self.warnings += 1;
        warn(m);
    }
    fn fail(&mut self, m: impl AsRef<str>) {
        self.failures += 1;
        fail(m);
    }
    fn check(&mut self, good: bool, yes: impl AsRef<str>, no: impl AsRef<str>) {
        if good {
            self.ok(yes)
        } else {
            self.fail(no)
        }
    }
}

pub async fn run(a: DoctorArgs) -> Result<()> {
    let dir = a.common.dir();
    let mut r = Report::default();

    title("Instalación");
    let env = match EnvFile::load(&dir.join(".env")) {
        Ok(e) => {
            r.ok(format!("Configuración: {}", dir.join(".env").display()));
            e
        }
        Err(_) => {
            r.fail(format!(
                "No hay .env en {} (¿instalado? usa --dir o rvmail install)",
                dir.display()
            ));
            return finish(r);
        }
    };
    for k in [
        "POSTGRES_PASSWORD",
        "REDIS_PASSWORD",
        "JWT_SECRET",
        "MASTER_KEY",
    ] {
        if env.value(k).is_none() {
            r.fail(format!("{k} está vacío en .env"));
        }
    }
    if let Some(mk) = env.value("MASTER_KEY") {
        use base64::Engine;
        let okk = base64::engine::general_purpose::STANDARD
            .decode(mk)
            .is_ok_and(|b| b.len() == 32);
        r.check(
            okk,
            "MASTER_KEY válida (32 bytes)",
            "MASTER_KEY no son 32 bytes en base64",
        );
    }
    if let Ok(meta) = std::fs::metadata(dir.join(".env")) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                r.warn("El .env lo pueden leer otros usuarios: chmod 600 .env");
            }
        }
        let _ = meta;
    }

    match engine::Engine::detect("") {
        Ok(e) => r.ok(format!("Contenedores: {}", e.label())),
        Err(e) => r.fail(e.to_string()),
    }
    let port = env.get_or("HTTP_PORT", "8088");
    let health = local_health(&port).await;
    match &health {
        Some(v) => r.ok(format!("API sana en 127.0.0.1:{port} (versión {v})")),
        None => r.fail(format!(
            "La API no responde en 127.0.0.1:{port}: docker compose ps / logs app"
        )),
    }
    disk(&mut r, &dir);
    backups(&mut r, &dir);

    title("Dominio y HTTPS");
    let domain = env.get_or("DOMAIN", "");
    let public_ip = if a.offline {
        None
    } else {
        detect::public_ip().await
    };
    if !a.offline {
        dns_points_here(&mut r, &domain, public_ip).await;
        https(&mut r, &domain).await;
    }
    if env.get_or("PROXY_MODE", "") == "caddy" {
        r.check(
            detect::port_busy(80) && detect::port_busy(443),
            "Caddy escucha en 80/443",
            "Nada escucha en 80/443 (¿Caddy caído o puertos bloqueados?)",
        );
    }

    let provider = env.get_or("MAILSERVER_PROVIDER", "none");
    if provider != "none" {
        title(format!("Servidor de correo ({provider})"));
        mailserver(&mut r, &env, &provider, public_ip, a.offline).await;
    }
    finish(r)
}

fn finish(r: Report) -> Result<()> {
    println!();
    match (r.failures, r.warnings) {
        (0, 0) => ok("Todo en orden"),
        (0, w) => warn(format!("{w} aviso(s), ningún error")),
        (f, w) => {
            fail(format!("{f} error(es) y {w} aviso(s)"));
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn local_health(port: &str) -> Option<String> {
    let http = crate::net::tls::http_client(false);
    let req = http
        .get(format!("http://127.0.0.1:{port}/api/health"))
        .send();
    let res = tokio::time::timeout(Duration::from_secs(5), req)
        .await
        .ok()?
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let v: serde_json::Value = res.json().await.ok()?;
    Some(v["version"].as_str().unwrap_or("?").to_string())
}

fn disk(r: &mut Report, dir: &std::path::Path) {
    let out = engine::output("df", &["-Pk", &dir.to_string_lossy()]);
    let avail_kb: Option<u64> = out
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|v| v.parse().ok());
    if let Some(kb) = avail_kb {
        let gb = kb as f64 / 1024.0 / 1024.0;
        if gb < 1.0 {
            r.fail(format!("Queda muy poco disco: {gb:.1} GB"));
        } else if gb < 5.0 {
            r.warn(format!("Disco libre: {gb:.1} GB"));
        } else {
            r.ok(format!("Disco libre: {gb:.0} GB"));
        }
    }
}

fn backups(r: &mut Report, dir: &std::path::Path) {
    let newest = std::fs::read_dir(dir.join("backups"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".dump"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max();
    match newest.and_then(|t| t.elapsed().ok()) {
        Some(age) if age < Duration::from_secs(2 * 86_400) => {
            r.ok(format!("Último respaldo hace {} h", age.as_secs() / 3600))
        }
        Some(age) => r.warn(format!(
            "El último respaldo tiene {} días: programa `rvmail backup` en cron",
            age.as_secs() / 86_400
        )),
        None => r.warn("No hay respaldos en backups/: programa `rvmail backup` en cron"),
    }
}

async fn dns_points_here(r: &mut Report, name: &str, public_ip: Option<IpAddr>) {
    if name.is_empty() {
        return;
    }
    let ips = detect::resolve(name).await;
    match (ips.is_empty(), public_ip) {
        (true, _) => r.fail(format!("{name} no resuelve: crea el registro A")),
        (false, Some(ip)) if ips.contains(&ip) => r.ok(format!("{name} → {ip}")),
        (false, Some(ip)) => r.warn(format!(
            "{name} → {} (este servidor es {ip}; normal si usas el proxy de Cloudflare)",
            ips.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        (false, None) => r.ok(format!("{name} resuelve")),
    }
}

async fn https(r: &mut Report, domain: &str) {
    if domain.is_empty() {
        return;
    }
    let http = crate::net::tls::http_client(false);
    let res = tokio::time::timeout(
        Duration::from_secs(10),
        http.get(format!("https://{domain}/api/health")).send(),
    )
    .await;
    match res {
        Ok(Ok(res)) if res.status().is_success() => {
            r.ok(format!("https://{domain} responde con certificado válido"))
        }
        Ok(Ok(res)) => r.fail(format!(
            "https://{domain} responde {} (¿proxy mal configurado?)",
            res.status()
        )),
        Ok(Err(e)) if e.is_connect() => r.fail(format!(
            "https://{domain}: sin conexión o certificado no válido"
        )),
        _ => r.fail(format!("https://{domain} no responde")),
    }
}

async fn tcp_open(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

async fn mailserver(
    r: &mut Report,
    env: &EnvFile,
    provider: &str,
    public_ip: Option<IpAddr>,
    offline: bool,
) {
    let host = env.get_or("MAILSERVER_HOSTNAME", "");
    if host.is_empty() {
        r.fail("MAILSERVER_HOSTNAME está vacío");
        return;
    }
    for (port, what) in [
        (25u16, "SMTP"),
        (465, "SMTPS"),
        (587, "Submission"),
        (993, "IMAPS"),
    ] {
        let open = tcp_open(SocketAddr::from(([127, 0, 0, 1], port))).await;
        r.check(
            open,
            format!("{what} escucha en el puerto {port}"),
            format!("Nada escucha en el puerto {port} ({what})"),
        );
    }
    match provider {
        "aapanel" => {
            let url = env
                .get_or("AAPANEL_URL", "")
                .replace("host.docker.internal", "127.0.0.1");
            let http = crate::net::tls::http_client(true);
            let up = tokio::time::timeout(Duration::from_secs(6), http.get(&url).send())
                .await
                .is_ok_and(|r| r.is_ok());
            r.check(
                up,
                format!("Panel de aaPanel accesible ({url})"),
                format!("aaPanel no responde en {url}"),
            );
            if env.value("AAPANEL_API_KEY").is_none() {
                r.fail("Falta AAPANEL_API_KEY: rvmail config mailserver");
            }
        }
        "stalwart" => {
            r.check(
                stalwart_setup::admin_works(env).await,
                "Administración de Stalwart con las credenciales de .env",
                "Stalwart no acepta STALWART_ADMIN_USER/PASSWORD (¿sin configurar? rvmail config mailserver)",
            );
        }
        _ => {}
    }
    if offline {
        return;
    }
    dns_points_here(r, &host, public_ip).await;
    let mail_domain = env.get_or("RVMAIL_MAIL_DOMAIN", detect::parent_domain(&host));
    let mx = detect::mx(&mail_domain).await;
    if mx.iter().any(|m| m == &host) {
        r.ok(format!("MX de {mail_domain} → {host}"));
    } else if mx.is_empty() {
        r.fail(format!("{mail_domain} no tiene registro MX (10 {host})"));
    } else {
        r.warn(format!(
            "MX de {mail_domain}: {} (se esperaba {host})",
            mx.join(", ")
        ));
    }
    let txt = detect::txt(&mail_domain).await;
    r.check(
        txt.iter().any(|t| t.starts_with("v=spf1")),
        format!("SPF de {mail_domain}"),
        format!("{mail_domain} sin SPF (TXT v=spf1 mx a ~all)"),
    );
    let dmarc = detect::txt(&format!("_dmarc.{mail_domain}")).await;
    if dmarc.iter().any(|t| t.starts_with("v=DMARC1")) {
        r.ok(format!("DMARC de {mail_domain}"));
    } else {
        r.warn(format!(
            "{mail_domain} sin DMARC (TXT _dmarc: v=DMARC1; p=quarantine)"
        ));
    }
    if let Some(ip) = public_ip {
        ptr(r, ip, &host).await;
    }
    // Muchos proveedores (Oracle, AWS, GCP…) bloquean la salida por el 25
    let out25 = match detect::resolve("gmail-smtp-in.l.google.com").await.first() {
        Some(ip) => tcp_open(SocketAddr::new(*ip, 25)).await,
        None => false,
    };
    if out25 {
        r.ok("Salida por el puerto 25 abierta (se puede entregar correo)");
    } else {
        r.fail("La salida por el puerto 25 está bloqueada: pide a tu proveedor que la abra o usa un relay SMTP");
    }
}

async fn ptr(r: &mut Report, ip: IpAddr, host: &str) {
    let Some(res) = detect::resolver() else {
        return;
    };
    let names: Vec<String> =
        match tokio::time::timeout(Duration::from_secs(5), res.reverse_lookup(ip)).await {
            Ok(Ok(l)) => l
                .answers()
                .iter()
                .filter_map(|rec| match &rec.data {
                    hickory_resolver::proto::rr::RData::PTR(p) => {
                        Some(p.0.to_string().trim_end_matches('.').to_lowercase())
                    }
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
    if names.iter().any(|n| n == host) {
        r.ok(format!("PTR de {ip} → {host}"));
    } else {
        r.warn(format!(
            "PTR de {ip} es {} (debería ser {host}; se cambia en el panel de tu proveedor del VPS)",
            if names.is_empty() {
                "inexistente".to_string()
            } else {
                names.join(", ")
            }
        ));
    }
}
