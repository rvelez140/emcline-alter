//! Autodetección del VPS: arquitectura, memoria, puertos ocupados, aaPanel,
//! Postfix, IP pública y DNS del dominio.

use hickory_resolver::proto::rr::RData;
use hickory_resolver::TokioResolver;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

pub const PANEL_DIR: &str = "/www/server/panel";

pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Memoria total en MB (`/proc/meminfo`).
pub fn mem_total_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

pub fn cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Puertos TCP en escucha en el host (lee `/proc/net/tcp` y `tcp6`; estado 0A = LISTEN).
pub fn listening_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        ports.extend(parse_proc_net_tcp(&text));
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn parse_proc_net_tcp(text: &str) -> Vec<u16> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            if cols.get(3) != Some(&"0A") {
                return None;
            }
            let port = cols.get(1)?.rsplit(':').next()?;
            u16::from_str_radix(port, 16).ok()
        })
        .collect()
}

pub fn port_busy(port: u16) -> bool {
    listening_ports().contains(&port)
}

#[derive(Debug, Default, Clone)]
pub struct AaPanel {
    pub installed: bool,
    pub port: u16,
    pub mail_server: bool,
}

pub fn aapanel() -> AaPanel {
    let dir = Path::new(PANEL_DIR);
    if !dir.exists() {
        return AaPanel::default();
    }
    let port = std::fs::read_to_string(dir.join("data/port.pl"))
        .ok()
        .and_then(|s| {
            s.chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(7800);
    AaPanel {
        installed: true,
        port,
        mail_server: dir.join("plugin/mail_sys").exists(),
    }
}

/// `myhostname` de Postfix (el nombre del certificado TLS del correo).
pub fn postfix_hostname() -> Option<String> {
    let from_postconf = super::engine::output("postconf", &["-h", "myhostname"]);
    let host = if from_postconf.is_empty() {
        std::fs::read_to_string("/etc/postfix/main.cf")
            .ok()
            .and_then(|cf| {
                cf.lines()
                    .filter_map(|l| l.trim().strip_prefix("myhostname"))
                    .filter_map(|r| r.trim_start().strip_prefix('='))
                    .map(|v| v.trim().to_string())
                    .next_back()
            })?
    } else {
        from_postconf
    };
    host.contains('.').then_some(host)
}

/// IP pública del VPS consultando servicios conocidos.
pub async fn public_ip() -> Option<IpAddr> {
    let http = crate::net::tls::http_client(false);
    for url in [
        "https://api.ipify.org",
        "https://ifconfig.me/ip",
        "https://icanhazip.com",
    ] {
        let fut = async { http.get(url).send().await.ok()?.text().await.ok() };
        if let Ok(Some(text)) = tokio::time::timeout(Duration::from_secs(5), fut).await {
            if let Ok(ip) = text.trim().parse() {
                return Some(ip);
            }
        }
    }
    None
}

pub fn resolver() -> Option<TokioResolver> {
    TokioResolver::builder_tokio().ok()?.build().ok()
}

/// Direcciones A/AAAA del nombre.
pub async fn resolve(name: &str) -> Vec<IpAddr> {
    let Some(r) = resolver() else { return vec![] };
    match tokio::time::timeout(Duration::from_secs(5), r.lookup_ip(name)).await {
        Ok(Ok(l)) => l.iter().collect(),
        _ => vec![],
    }
}

pub async fn mx(domain: &str) -> Vec<String> {
    let Some(r) = resolver() else { return vec![] };
    match tokio::time::timeout(Duration::from_secs(5), r.mx_lookup(domain)).await {
        Ok(Ok(l)) => l
            .answers()
            .iter()
            .filter_map(|rec| match &rec.data {
                RData::MX(m) => Some(m.exchange.to_string().trim_end_matches('.').to_lowercase()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

pub async fn txt(name: &str) -> Vec<String> {
    let Some(r) = resolver() else { return vec![] };
    match tokio::time::timeout(Duration::from_secs(5), r.txt_lookup(name)).await {
        Ok(Ok(l)) => l
            .answers()
            .iter()
            .filter_map(|rec| match &rec.data {
                RData::TXT(t) => Some(
                    t.txt_data
                        .iter()
                        .map(|p| String::from_utf8_lossy(p))
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// Perfil de recursos recomendado según la memoria del VPS (fase H5).
pub struct Profile {
    pub name: &'static str,
    pub vars: &'static [(&'static str, &'static str)],
}

pub fn profile_for(mem_mb: u64) -> Profile {
    if mem_mb > 0 && mem_mb < 1536 {
        Profile {
            name: "pequeño (≤1 GB)",
            vars: &[
                ("APP_MEM_LIMIT", "160m"),
                ("WORKER_MEM_LIMIT", "160m"),
                ("PG_MEM_LIMIT", "384m"),
                ("REDIS_MEM_LIMIT", "160m"),
                ("REDIS_MAXMEMORY", "128mb"),
                ("PG_SHARED_BUFFERS", "64MB"),
                ("PG_EFFECTIVE_CACHE", "192MB"),
                ("PG_MAX_CONNECTIONS", "30"),
                ("WORKER_MAX_ACCOUNTS", "100"),
                ("STALWART_MEM_LIMIT", "384m"),
            ],
        }
    } else if mem_mb >= 6144 {
        Profile {
            name: "grande (≥6 GB)",
            vars: &[
                ("APP_MEM_LIMIT", "512m"),
                ("WORKER_MEM_LIMIT", "512m"),
                ("PG_MEM_LIMIT", "2g"),
                ("REDIS_MEM_LIMIT", "640m"),
                ("REDIS_MAXMEMORY", "512mb"),
                ("PG_SHARED_BUFFERS", "512MB"),
                ("PG_EFFECTIVE_CACHE", "1536MB"),
                ("PG_MAX_CONNECTIONS", "100"),
                ("WORKER_MAX_ACCOUNTS", "2000"),
                ("STALWART_MEM_LIMIT", "1g"),
            ],
        }
    } else {
        Profile {
            name: "estándar (2–4 GB)",
            vars: &[
                ("APP_MEM_LIMIT", "256m"),
                ("WORKER_MEM_LIMIT", "256m"),
                ("PG_MEM_LIMIT", "768m"),
                ("REDIS_MEM_LIMIT", "320m"),
                ("REDIS_MAXMEMORY", "256mb"),
                ("PG_SHARED_BUFFERS", "128MB"),
                ("PG_EFFECTIVE_CACHE", "384MB"),
                ("PG_MAX_CONNECTIONS", "50"),
                ("WORKER_MAX_ACCOUNTS", "500"),
                ("STALWART_MEM_LIMIT", "512m"),
            ],
        }
    }
}

pub fn valid_domain(d: &str) -> bool {
    let parts: Vec<&str> = d.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        && parts
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
}

/// `correo.ejemplo.com` → `ejemplo.com` (para proponer `mail.ejemplo.com`).
pub fn parent_domain(d: &str) -> &str {
    match d.split_once('.') {
        Some((_, rest)) if rest.contains('.') => rest,
        _ => d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listening_ports() {
        let text = "  sl  local_address rem_address   st\n\
           0: 00000000:0050 00000000:0000 0A 00000000:00000000\n\
           1: 0100007F:1F90 0100007F:D2A4 01 00000000:00000000\n\
           2: 00000000000000000000000000000000:01BB 00000000000000000000000000000000:0000 0A 0\n";
        assert_eq!(parse_proc_net_tcp(text), vec![80, 443]);
    }

    #[test]
    fn domains() {
        assert!(valid_domain("correo.solucionesrv.net"));
        assert!(!valid_domain("localhost"));
        assert!(!valid_domain("-a.com"));
        assert_eq!(parent_domain("correo.solucionesrv.net"), "solucionesrv.net");
        assert_eq!(parent_domain("solucionesrv.net"), "solucionesrv.net");
    }

    #[test]
    fn profiles() {
        assert!(profile_for(1024).name.starts_with("pequeño"));
        assert!(profile_for(4096).name.starts_with("estándar"));
        assert!(profile_for(24576).name.starts_with("grande"));
    }
}
