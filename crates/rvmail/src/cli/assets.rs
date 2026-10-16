//! Archivos de despliegue incluidos en el binario: con solo `rvmail` (instalador
//! de una línea) se puede montar la instalación sin clonar el repositorio.

use anyhow::Result;
use std::path::Path;

pub const COMPOSE: &str = include_str!("../../../../docker-compose.yml");
pub const COMPOSE_HA: &str = include_str!("../../../../docker-compose.ha.yml");
pub const ENV_EXAMPLE: &str = include_str!("../../../../.env.example");
pub const CADDYFILE: &str = include_str!("../../../../deploy/Caddyfile");
pub const NGINX_EXAMPLE: &str = include_str!("../../../../deploy/nginx-aapanel.conf.example");

/// Escribe los archivos que falten. Con `refresh`, actualiza también los que ya
/// existen (no se toca `.env`). En un clon del repo con `build:` local no se
/// sobrescribe nada: manda el repositorio.
pub fn write(dir: &Path, refresh: bool) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir.join("deploy"))?;
    let is_repo = dir.join(".git").exists();
    let files = [
        ("docker-compose.yml", COMPOSE),
        ("docker-compose.ha.yml", COMPOSE_HA),
        ("deploy/Caddyfile", CADDYFILE),
        ("deploy/nginx-aapanel.conf.example", NGINX_EXAMPLE),
        (".env.example", ENV_EXAMPLE),
    ];
    let mut written = Vec::new();
    for (name, content) in files {
        let path = dir.join(name);
        let exists = path.exists();
        if exists && (!refresh || is_repo) {
            continue;
        }
        if exists && std::fs::read_to_string(&path).is_ok_and(|c| c == content) {
            continue;
        }
        std::fs::write(&path, content)?;
        written.push(name.to_string());
    }
    Ok(written)
}
