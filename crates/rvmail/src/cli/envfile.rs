//! Lectura y escritura del `.env` conservando comentarios y orden (como `set_env`
//! del instalador en bash). Siempre se guarda con permisos 600.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct EnvFile {
    path: PathBuf,
    lines: Vec<String>,
}

/// Variables que nunca se muestran completas.
pub fn is_secret(key: &str) -> bool {
    ["PASSWORD", "SECRET", "MASTER_KEY", "API_KEY", "TOKEN"]
        .iter()
        .any(|s| key.contains(s))
}

pub fn mask(key: &str, value: &str) -> String {
    if value.is_empty() || !is_secret(key) {
        return value.to_string();
    }
    format!("•••••• ({} caracteres)", value.chars().count())
}

impl EnvFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("No se pudo leer {}", path.display()))?;
        Ok(Self::parse(path, &text))
    }

    pub fn parse(path: &Path, text: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            lines: text.lines().map(str::to_string).collect(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn find(&self, key: &str) -> Option<usize> {
        let prefix = format!("{key}=");
        self.lines.iter().position(|l| l.starts_with(&prefix))
    }

    /// Valor de la variable (`None` si no existe; vacío si existe sin valor).
    pub fn get(&self, key: &str) -> Option<String> {
        self.find(key)
            .map(|i| unquote(&self.lines[i][key.len() + 1..]))
    }

    /// Valor no vacío.
    pub fn value(&self, key: &str) -> Option<String> {
        self.get(key).filter(|v| !v.is_empty())
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.value(key).unwrap_or_else(|| default.to_string())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}={}", quote(value));
        match self.find(key) {
            Some(i) => self.lines[i] = line,
            None => self.lines.push(line),
        }
    }

    /// Fija la variable solo si está vacía o no existe (no pisa secretos ya generados).
    pub fn set_default(&mut self, key: &str, value: &str) {
        if self.value(key).is_none() {
            self.set(key, value);
        }
    }

    /// Pares clave/valor en el orden del archivo.
    pub fn entries(&self) -> Vec<(String, String)> {
        self.lines
            .iter()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), unquote(v)))
            .collect()
    }

    pub fn save(&self) -> Result<()> {
        let mut text = self.lines.join("\n");
        text.push('\n');
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, text)
            .with_context(|| format!("No se pudo escribir {}", tmp.display()))?;
        set_private(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
pub fn set_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn set_private(_: &Path) -> Result<()> {
    Ok(())
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// docker compose interpreta `$` en el `.env`; los valores con espacios o `#` van entre comillas simples.
fn quote(v: &str) -> String {
    if v.chars()
        .any(|c| c.is_whitespace() || c == '#' || c == '$' || c == '"')
        && !v.contains('\'')
    {
        format!("'{v}'")
    } else {
        v.to_string()
    }
}

/// Secreto aleatorio en hexadecimal (`bytes` bytes de entropía).
pub fn gen_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// 32 bytes en base64 (formato de `MASTER_KEY`).
pub fn gen_master_key() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_comments_and_order() {
        let mut env = EnvFile::parse(Path::new("x"), "# hola\nA=1\n\nB=\nC='con espacio'\n");
        assert_eq!(env.get("A").as_deref(), Some("1"));
        assert_eq!(env.get("B").as_deref(), Some(""));
        assert_eq!(env.value("B"), None);
        assert_eq!(env.get("C").as_deref(), Some("con espacio"));
        env.set("B", "2");
        env.set_default("A", "9");
        env.set("D", "a b");
        assert_eq!(
            env.lines,
            ["# hola", "A=1", "", "B=2", "C='con espacio'", "D='a b'"]
        );
    }

    #[test]
    fn masks_secrets() {
        assert_eq!(mask("DOMAIN", "x.com"), "x.com");
        assert!(mask("JWT_SECRET", "abc").starts_with("••"));
        assert_eq!(mask("AAPANEL_API_KEY", ""), "");
        assert_eq!(gen_hex(24).len(), 48);
    }
}
