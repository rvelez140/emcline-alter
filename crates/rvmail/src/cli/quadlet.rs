//! Podman Quadlet: genera unidades systemd (`/etc/containers/systemd/*.container`)
//! equivalentes a `docker-compose.yml`, para servidores sin Docker.
//!
//! Quadlet no interpola variables, así que se escribe un archivo de entorno ya
//! resuelto (`quadlet/rvmail.env`, permisos 600) con `DATABASE_URL`, etc.

use super::envfile::{set_private, EnvFile};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const UNIT_DIR: &str = "/etc/containers/systemd";
pub const TARGET: &str = "/etc/systemd/system/rvmail.target";

pub struct Unit {
    pub file: String,
    pub content: String,
}

fn profiles(env: &EnvFile) -> Vec<String> {
    env.get_or("COMPOSE_PROFILES", "")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Servicios activos según los perfiles.
pub fn services(env: &EnvFile) -> Vec<&'static str> {
    let p = profiles(env);
    let mut s = vec!["postgres", "redis", "app", "worker"];
    if p.iter().any(|x| x == "stalwart") {
        s.push("stalwart");
    }
    if p.iter().any(|x| x == "caddy") {
        s.push("caddy");
    }
    s
}

/// Entorno resuelto (lo que compose calcularía en `x-app-env`).
pub fn resolved_env(env: &EnvFile) -> Vec<(String, String)> {
    let g = |k: &str, d: &str| env.get_or(k, d);
    let mut vars: Vec<(String, String)> = env
        .entries()
        .into_iter()
        .filter(|(k, _)| k != "COMPOSE_PROFILES")
        .collect();
    let mut put = |k: &str, v: String| {
        vars.retain(|(x, _)| x != k);
        vars.push((k.to_string(), v));
    };
    put("RVMAIL_ENV", "production".into());
    put("LOG_FORMAT", "json".into());
    put(
        "DATABASE_URL",
        format!(
            "postgres://{}:{}@rvmail-postgres:5432/{}",
            g("POSTGRES_USER", "rvmail"),
            g("POSTGRES_PASSWORD", ""),
            g("POSTGRES_DB", "rvmail")
        ),
    );
    put(
        "REDIS_URL",
        format!("redis://:{}@rvmail-redis:6379", g("REDIS_PASSWORD", "")),
    );
    put("APP_URL", format!("https://{}", g("DOMAIN", "localhost")));
    if g("STALWART_URL", "http://stalwart:8080") == "http://stalwart:8080" {
        put("STALWART_URL", "http://rvmail-stalwart:8080".into());
    }
    put(
        "STALWART_RECOVERY_ADMIN",
        format!("admin:{}", g("STALWART_RECOVERY_PASSWORD", "")),
    );
    vars
}

pub fn units(env: &EnvFile, dir: &Path) -> Vec<Unit> {
    let g = |k: &str, d: &str| env.get_or(k, d);
    let env_file = dir.join("quadlet/rvmail.env");
    let env_file = env_file.display();
    let image = format!(
        "{}:{}",
        g("IMAGE_PREFIX", "ghcr.io/rvelez140/rvmail"),
        g("APP_VERSION", "latest")
    );
    let mail_host = g("MAILSERVER_HOSTNAME", "mail.invalid");
    let header = |desc: &str, after: &str| {
        format!(
            "# Generado por `rvmail install --engine quadlet`. No editar: usa `rvmail config`.\n\
             [Unit]\nDescription=RV Mail — {desc}\nPartOf=rvmail.target\n{after}\n"
        )
    };
    let footer =
        "[Service]\nRestart=always\nTimeoutStartSec=300\n\n[Install]\nWantedBy=rvmail.target\n";
    let hardening = "ReadOnly=true\nTmpfs=/tmp\nNoNewPrivileges=true\nDropCapability=ALL\n";
    let host_access =
        format!("AddHost=host.docker.internal:host-gateway\nAddHost={mail_host}:host-gateway\n");
    let deps = "Requires=rvmail-postgres.service rvmail-redis.service\nAfter=rvmail-postgres.service rvmail-redis.service";

    let mut out = vec![
        Unit {
            file: "rvmail.network".into(),
            content: format!(
                "[Network]\nNetworkName=rvmail\nSubnet={}\n",
                g("DOCKER_SUBNET", "172.30.57.0/24")
            ),
        },
        Unit {
            file: "rvmail-postgres.container".into(),
            content: format!(
                "{}[Container]\nContainerName=rvmail-postgres\nImage=docker.io/library/postgres:16-alpine\n\
                 Network=rvmail.network\nEnvironmentFile={env_file}\nVolume=rvmail-postgres-data:/var/lib/postgresql/data\n\
                 ShmSize=256m\nPodmanArgs=--memory {}\n\
                 Exec=postgres -c shared_buffers={} -c effective_cache_size={} -c max_connections={} -c wal_level=replica -c max_wal_senders=4 -c hot_standby=on\n\
                 HealthCmd=pg_isready -U {} -d {}\nHealthInterval=10s\nNotify=healthy\n\n{footer}",
                header("PostgreSQL", ""),
                g("PG_MEM_LIMIT", "768m"),
                g("PG_SHARED_BUFFERS", "128MB"),
                g("PG_EFFECTIVE_CACHE", "384MB"),
                g("PG_MAX_CONNECTIONS", "50"),
                g("POSTGRES_USER", "rvmail"),
                g("POSTGRES_DB", "rvmail"),
            ),
        },
        Unit {
            file: "rvmail-redis.container".into(),
            content: format!(
                "{}[Container]\nContainerName=rvmail-redis\nImage=docker.io/library/redis:7-alpine\n\
                 Network=rvmail.network\nVolume=rvmail-redis-data:/data\nPodmanArgs=--memory {}\n\
                 Exec=redis-server --requirepass {} --appendonly yes --maxmemory {} --maxmemory-policy noeviction\n\
                 HealthCmd=redis-cli -a {} --no-auth-warning ping\nHealthInterval=10s\nNotify=healthy\n\n{footer}",
                header("Redis", ""),
                g("REDIS_MEM_LIMIT", "320m"),
                g("REDIS_PASSWORD", ""),
                g("REDIS_MAXMEMORY", "256mb"),
                g("REDIS_PASSWORD", ""),
            ),
        },
        Unit {
            file: "rvmail-app.container".into(),
            content: format!(
                "{}[Container]\nContainerName=rvmail-app\nImage={image}\nExec=api\n\
                 Network=rvmail.network:ip={}\nEnvironmentFile={env_file}\n\
                 PublishPort={}:{}:3000\n{hardening}{host_access}PodmanArgs=--memory {}\n\
                 HealthCmd=/usr/local/bin/rvmail healthcheck api\nHealthInterval=15s\nNotify=healthy\n\n{footer}",
                header("API y web", deps),
                g("API_IP", "172.30.57.10"),
                g("HTTP_BIND", "127.0.0.1"),
                g("HTTP_PORT", "8088"),
                g("APP_MEM_LIMIT", "256m"),
            ),
        },
        Unit {
            file: "rvmail-worker.container".into(),
            content: format!(
                "{}[Container]\nContainerName=rvmail-worker\nImage={image}\nExec=worker\n\
                 Network=rvmail.network\nEnvironmentFile={env_file}\n{hardening}{host_access}PodmanArgs=--memory {}\n\
                 HealthCmd=/usr/local/bin/rvmail healthcheck worker\nHealthInterval=30s\n\n{footer}",
                header("sincronización de buzones", "Requires=rvmail-app.service\nAfter=rvmail-app.service"),
                g("WORKER_MEM_LIMIT", "256m"),
            ),
        },
    ];
    let svcs = services(env);
    if svcs.contains(&"stalwart") {
        out.push(Unit {
            file: "rvmail-stalwart.container".into(),
            content: format!(
                "{}[Container]\nContainerName=rvmail-stalwart\nImage=docker.io/stalwartlabs/stalwart:{}\n\
                 Network=rvmail.network\nEnvironment=STALWART_RECOVERY_ADMIN=admin:{}\n\
                 PublishPort=25:25\nPublishPort=465:465\nPublishPort=587:587\nPublishPort=993:993\nPublishPort=4190:4190\n\
                 PublishPort=127.0.0.1:{}:8080\n\
                 Volume=rvmail-stalwart-data:/var/lib/stalwart\nVolume=rvmail-stalwart-etc:/etc/stalwart\nPodmanArgs=--memory {}\n\n{footer}",
                header("servidor de correo Stalwart", ""),
                g("STALWART_VERSION", "latest"),
                g("STALWART_RECOVERY_PASSWORD", ""),
                g("STALWART_HTTP_PORT", "8180"),
                g("STALWART_MEM_LIMIT", "512m"),
            ),
        });
    }
    if svcs.contains(&"caddy") {
        out.push(Unit {
            file: "rvmail-caddy.container".into(),
            content: format!(
                "{}[Container]\nContainerName=rvmail-caddy\nImage=docker.io/library/caddy:2-alpine\n\
                 Network=rvmail.network\nEnvironment=DOMAIN={} ACME_EMAIL={}\n\
                 PublishPort=80:80\nPublishPort=443:443\nPublishPort=443:443/udp\n\
                 Volume={}:/etc/caddy/Caddyfile:ro,Z\nVolume=rvmail-caddy-data:/data\nVolume=rvmail-caddy-config:/config\n\n{footer}",
                header("proxy HTTPS (Caddy)", "Requires=rvmail-app.service\nAfter=rvmail-app.service"),
                g("DOMAIN", "localhost"),
                g("ACME_EMAIL", ""),
                dir.join("quadlet/Caddyfile").display(),
            ),
        });
    }
    out
}

pub fn target(services: &[&str]) -> String {
    let wants: Vec<String> = services
        .iter()
        .map(|s| format!("rvmail-{s}.service"))
        .collect();
    format!(
        "# Generado por rvmail. Arranca y detiene todo RV Mail.\n[Unit]\nDescription=RV Mail\nWants={}\n\n[Install]\nWantedBy=multi-user.target\n",
        wants.join(" ")
    )
}

/// Escribe las unidades, el entorno resuelto y el Caddyfile; recarga systemd.
pub fn write_all(dir: &Path) -> Result<()> {
    let env = EnvFile::load(&dir.join(".env"))?;
    let qdir = dir.join("quadlet");
    std::fs::create_dir_all(&qdir)?;
    let env_path = qdir.join("rvmail.env");
    let body: String = resolved_env(&env)
        .into_iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    std::fs::write(&env_path, body)?;
    set_private(&env_path)?;
    let caddy = std::fs::read_to_string(dir.join("deploy/Caddyfile")).unwrap_or_default();
    std::fs::write(
        qdir.join("Caddyfile"),
        caddy.replace("app:3000", "rvmail-app:3000"),
    )?;

    std::fs::create_dir_all(UNIT_DIR)
        .context("No se pudo crear /etc/containers/systemd (¿ejecutas como root?)")?;
    // Borra unidades de servicios que ya no se usan (p. ej. se quitó Stalwart)
    if let Ok(rd) = std::fs::read_dir(UNIT_DIR) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("rvmail") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for u in units(&env, dir) {
        std::fs::write(PathBuf::from(UNIT_DIR).join(&u.file), u.content)?;
    }
    std::fs::write(TARGET, target(&services(&env)))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "rvmail.target"])
}

fn systemctl(args: &[&str]) -> Result<()> {
    let ok = Command::new("systemctl")
        .args(args)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        bail!("Falló: systemctl {}", args.join(" "));
    }
    Ok(())
}

/// Traduce las órdenes de compose que usa el CLI a systemd.
pub fn compose_like(dir: &Path, args: &[&str]) -> Result<()> {
    let env = EnvFile::load(&dir.join(".env"))?;
    let named: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|s| format!("rvmail-{s}.service"))
        .collect();
    let named: Vec<&str> = named.iter().map(String::as_str).collect();
    match args.first().copied() {
        Some("up") => {
            write_all(dir)?;
            systemctl(&["start", "rvmail.target"])?;
            systemctl(&["try-restart", "rvmail-app.service", "rvmail-worker.service"])
        }
        Some("pull") => {
            let images: Vec<String> = units(&env, dir)
                .iter()
                .flat_map(|u| {
                    u.content
                        .lines()
                        .filter_map(|l| l.strip_prefix("Image="))
                        .map(str::to_string)
                })
                .collect();
            for img in images {
                let ok = Command::new("podman")
                    .args(["pull", &img])
                    .status()
                    .is_ok_and(|s| s.success());
                if !ok {
                    bail!("No se pudo descargar {img}");
                }
            }
            Ok(())
        }
        Some("build") => {
            bail!("Con Quadlet se usan imágenes precompiladas: pon IMAGE_PREFIX=ghcr.io/… en .env")
        }
        Some(cmd @ ("restart" | "stop" | "start")) => {
            let mut a = vec![cmd];
            if named.is_empty() {
                a.push("rvmail.target");
            } else {
                a.extend(named);
            }
            systemctl(&a)
        }
        Some("down") => systemctl(&["stop", "rvmail.target"]),
        Some("ps") => {
            let _ = Command::new("systemctl")
                .args(["list-units", "rvmail*", "--no-pager"])
                .status();
            Ok(())
        }
        Some("logs") => {
            let mut c = Command::new("journalctl");
            c.args(["--no-pager", "-n", "200"]);
            for n in &named {
                c.args(["-u", n]);
            }
            if named.is_empty() {
                c.args(["-u", "rvmail-*"]);
            }
            let _ = c.status();
            Ok(())
        }
        other => bail!("Orden no soportada con Quadlet: {}", other.unwrap_or("")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(extra: &str) -> EnvFile {
        EnvFile::parse(
            Path::new(".env"),
            &format!(
                "DOMAIN=correo.x.com\nPOSTGRES_PASSWORD=pg\nREDIS_PASSWORD=rd\nIMAGE_PREFIX=ghcr.io/a/rvmail\nHTTP_PORT=8090\n{extra}"
            ),
        )
    }

    #[test]
    fn base_units() {
        let e = env("COMPOSE_PROFILES=");
        let u = units(&e, Path::new("/opt/rvmail"));
        let names: Vec<&str> = u.iter().map(|u| u.file.as_str()).collect();
        assert_eq!(
            names,
            [
                "rvmail.network",
                "rvmail-postgres.container",
                "rvmail-redis.container",
                "rvmail-app.container",
                "rvmail-worker.container"
            ]
        );
        let app = &u[3].content;
        assert!(app.contains("Image=ghcr.io/a/rvmail:latest"));
        assert!(app.contains("PublishPort=127.0.0.1:8090:3000"));
        assert!(app.contains("Network=rvmail.network:ip=172.30.57.10"));
        assert!(app.contains("EnvironmentFile=/opt/rvmail/quadlet/rvmail.env"));
    }

    #[test]
    fn profiles_add_units() {
        let e = env("COMPOSE_PROFILES=caddy,stalwart\nSTALWART_RECOVERY_PASSWORD=s3");
        let u = units(&e, Path::new("/opt/rvmail"));
        assert!(u.iter().any(|u| u.file == "rvmail-caddy.container"));
        let st = u
            .iter()
            .find(|u| u.file == "rvmail-stalwart.container")
            .unwrap();
        assert!(st.content.contains("STALWART_RECOVERY_ADMIN=admin:s3"));
        assert!(target(&services(&e)).contains("rvmail-caddy.service"));
    }

    #[test]
    fn resolves_urls() {
        let vars = resolved_env(&env("POSTGRES_USER=u\nPOSTGRES_DB=d"));
        let get = |k: &str| {
            vars.iter()
                .find(|(x, _)| x == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(
            get("DATABASE_URL"),
            "postgres://u:pg@rvmail-postgres:5432/d"
        );
        assert_eq!(get("REDIS_URL"), "redis://:rd@rvmail-redis:6379");
        assert_eq!(get("STALWART_URL"), "http://rvmail-stalwart:8080");
        assert_eq!(get("APP_URL"), "https://correo.x.com");
    }
}
