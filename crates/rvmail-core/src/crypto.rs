#![allow(deprecated)] // generic-array 0.14 (aes-gcm 0.10): from_slice marcado obsoleto, sin alternativa estable aún
//! Cifrado de credenciales de cuentas de correo (AES-256-GCM).
//! Formato almacenado: `v1:<iv b64>:<tag b64>:<ciphertext b64>`, idéntico
//! al de `packages/shared/src/crypto.ts` para que Node y Rust compartan BD.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand_core::{OsRng, RngCore};

const VERSION: &str = "v1";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("MASTER_KEY debe ser de 32 bytes en base64")]
    BadKey,
    #[error("Formato de secreto cifrado no reconocido")]
    BadFormat,
    #[error("No se pudo descifrar el secreto (¿MASTER_KEY distinta?)")]
    Decrypt,
}

/// Clave maestra ya decodificada. No implementa Debug para no filtrarla en logs.
#[derive(Clone)]
pub struct MasterKey(Key<Aes256Gcm>);

impl MasterKey {
    pub fn from_base64(s: &str) -> Result<Self, CryptoError> {
        let raw = B64.decode(s.trim()).map_err(|_| CryptoError::BadKey)?;
        if raw.len() != 32 {
            return Err(CryptoError::BadKey);
        }
        Ok(Self(Key::<Aes256Gcm>::clone_from_slice(&raw)))
    }

    pub fn encrypt(&self, plain: &str) -> String {
        let cipher = Aes256Gcm::new(&self.0);
        let mut iv = [0u8; 12];
        OsRng.fill_bytes(&mut iv);
        // aes-gcm devuelve ciphertext || tag (16 bytes)
        let mut out = cipher
            .encrypt(&Nonce::clone_from_slice(&iv), plain.as_bytes())
            .expect("AES-GCM no falla al cifrar");
        let tag = out.split_off(out.len() - 16);
        format!(
            "{VERSION}:{}:{}:{}",
            B64.encode(iv),
            B64.encode(tag),
            B64.encode(out)
        )
    }

    pub fn decrypt(&self, stored: &str) -> Result<String, CryptoError> {
        let mut parts = stored.split(':');
        let (Some(VERSION), Some(iv), Some(tag), Some(data), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(CryptoError::BadFormat);
        };
        let iv = B64.decode(iv).map_err(|_| CryptoError::BadFormat)?;
        let tag = B64.decode(tag).map_err(|_| CryptoError::BadFormat)?;
        let mut data = B64.decode(data).map_err(|_| CryptoError::BadFormat)?;
        if iv.len() != 12 || tag.len() != 16 {
            return Err(CryptoError::BadFormat);
        }
        data.extend_from_slice(&tag);
        let cipher = Aes256Gcm::new(&self.0);
        let plain = cipher
            .decrypt(&Nonce::clone_from_slice(&iv), data.as_ref())
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plain).map_err(|_| CryptoError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "q7l9H2v0wKx3m5n8p1r4t6y9z2a5c8e1g4j7k0m3o6s=";

    #[test]
    fn roundtrip() {
        let k = MasterKey::from_base64(KEY).unwrap();
        let enc = k.encrypt("contraseña secreta");
        assert!(enc.starts_with("v1:"));
        assert_eq!(k.decrypt(&enc).unwrap(), "contraseña secreta");
    }

    /// Valor generado con packages/shared/src/crypto.ts (Node) y esta misma clave.
    #[test]
    fn decrypts_node_format() {
        let k = MasterKey::from_base64(KEY).unwrap();
        let from_node = include_str!("../testdata/node-secret.txt").trim();
        assert_eq!(k.decrypt(from_node).unwrap(), "test1");
    }

    #[test]
    fn rejects_bad_key() {
        assert!(MasterKey::from_base64("corta").is_err());
    }
}
