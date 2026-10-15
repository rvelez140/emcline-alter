//! Motor de contenedores: Docker (compose v2) o Podman (`podman compose` /
//! `podman-compose`, o Quadlet + systemd).

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Docker,
    Podman,
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub kind: Kind,
    /// Comando de compose: `docker compose`, `podman compose` o `podman-compose`.
    /// Vacío con Quadlet (los servicios son unidades systemd).
    pub compose: Vec<String>,
    pub quadlet: bool,
}

pub fn has(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn works(cmd: &[&str]) -> bool {
    Command::new(cmd[0])
        .args(&cmd[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Salida estándar de un comando (vacía si falla).
pub fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

impl Engine {
    /// `preferred`: "docker", "podman", "quadlet" o "" (automático: Docker si existe).
    pub fn detect(preferred: &str) -> Result<Self> {
        let docker = || -> Option<Self> {
            (has("docker") && works(&["docker", "compose", "version"])).then(|| Self {
                kind: Kind::Docker,
                compose: vec!["docker".into(), "compose".into()],
                quadlet: false,
            })
        };
        let podman = || -> Option<Self> {
            if !has("podman") {
                return None;
            }
            if works(&["podman", "compose", "version"]) {
                Some(Self {
                    kind: Kind::Podman,
                    compose: vec!["podman".into(), "compose".into()],
                    quadlet: false,
                })
            } else if has("podman-compose") {
                Some(Self {
                    kind: Kind::Podman,
                    compose: vec!["podman-compose".into()],
                    quadlet: false,
                })
            } else {
                None
            }
        };
        let quadlet = || -> Option<Self> {
            (has("podman") && Path::new("/etc/containers").exists() && has("systemctl")).then(
                || Self {
                    kind: Kind::Podman,
                    compose: vec![],
                    quadlet: true,
                },
            )
        };
        let found = match preferred {
            "docker" => docker(),
            "podman" => podman(),
            "quadlet" => quadlet(),
            _ => docker().or_else(podman).or_else(quadlet),
        };
        match (found, preferred) {
            (Some(e), _) => Ok(e),
            (None, "docker") => {
                bail!("No encuentro Docker con el plugin compose v2 (`docker compose version`).")
            }
            (None, "podman") => {
                bail!("No encuentro `podman compose` ni `podman-compose`. Instala podman-compose o usa --engine quadlet.")
            }
            (None, "quadlet") => bail!("Quadlet necesita Podman 4.4+ y systemd."),
            (None, _) => bail!("No hay Docker ni Podman instalados."),
        }
    }

    pub fn bin(&self) -> &'static str {
        match self.kind {
            Kind::Docker => "docker",
            Kind::Podman => "podman",
        }
    }

    pub fn label(&self) -> String {
        let version = output(self.bin(), &["version", "--format", "{{.Server.Version}}"]);
        let version = if version.is_empty() {
            output(self.bin(), &["--version"])
        } else {
            version
        };
        let how = if self.quadlet {
            "Quadlet/systemd".to_string()
        } else {
            self.compose.join(" ")
        };
        format!("{} {} ({how})", self.bin(), version.trim())
    }

    /// Ejecuta `compose <args>` en `dir`, mostrando la salida.
    pub fn compose(&self, dir: &Path, args: &[&str]) -> Result<()> {
        if self.quadlet {
            return super::quadlet::compose_like(dir, args);
        }
        let status = Command::new(&self.compose[0])
            .args(&self.compose[1..])
            .args(args)
            .current_dir(dir)
            .status()
            .with_context(|| format!("No se pudo ejecutar {}", self.compose.join(" ")))?;
        if !status.success() {
            bail!("Falló: {} {}", self.compose.join(" "), args.join(" "));
        }
        Ok(())
    }

    /// `compose exec -T <service> <cmd…>` con stdin/stdout redirigidos (pg_dump, pg_restore).
    pub fn exec_cmd(&self, dir: &Path, service: &str, cmd: &[&str]) -> Command {
        if self.quadlet {
            let mut c = Command::new("podman");
            c.args(["exec", "-i", &format!("rvmail-{service}")])
                .args(cmd);
            return c;
        }
        let mut c = Command::new(&self.compose[0]);
        c.args(&self.compose[1..])
            .args(["exec", "-T", service])
            .args(cmd)
            .current_dir(dir);
        c
    }
}

/// Espera a que la API responda en `/api/health`.
pub async fn wait_healthy(port: &str, secs: u64) -> bool {
    let url = format!("http://127.0.0.1:{port}/api/health");
    let http = crate::net::tls::http_client(false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        let req =
            tokio::time::timeout(std::time::Duration::from_secs(3), http.get(&url).send()).await;
        if matches!(req, Ok(Ok(r)) if r.status().is_success()) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    false
}
