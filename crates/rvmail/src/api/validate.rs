//! Extractor JSON con errores legibles en español y validaciones comunes.

use super::error::ApiError;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;

/// Como `axum::Json`, pero un cuerpo inválido produce `400 Datos inválidos — …`.
pub struct Body<T>(pub T);

impl<S, T> FromRequest<S> for Body<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Body(v)),
            Err(JsonRejection::JsonDataError(e)) => Err(ApiError::bad_request(format!(
                "Datos inválidos — {}",
                spanish(&e.body_text())
            ))),
            Err(_) => Err(ApiError::bad_request(
                "Datos inválidos — el cuerpo debe ser JSON",
            )),
        }
    }
}

/// Traduce el error de serde a algo legible: "password: campo obligatorio".
fn spanish(raw: &str) -> String {
    let msg = raw.rsplit(": ").next().unwrap_or(raw);
    let msg = msg.split(" at line ").next().unwrap_or(msg);
    let path = raw
        .split(": ")
        .nth(1)
        .filter(|p| !p.contains(' '))
        .map(|p| format!("{p}: "))
        .unwrap_or_default();
    if let Some(f) = msg
        .strip_prefix("missing field `")
        .and_then(|r| r.strip_suffix('`'))
    {
        return format!("{f}: campo obligatorio");
    }
    if msg.starts_with("invalid type") || msg.starts_with("invalid value") {
        return format!("{path}tipo o valor no válido");
    }
    if msg.starts_with("unknown variant") {
        return format!("{path}valor no permitido");
    }
    if msg.starts_with("UUID parsing failed")
        || msg.contains("invalid character")
        || msg.contains("invalid length")
    {
        return format!("{path}identificador no válido");
    }
    format!("{path}{msg}")
}

/// Validación sencilla y estricta de correo (sin espacios, una @, dominio con punto).
pub fn is_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && domain.len() <= 253
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c == '<' || c == '>' || c == ',' || c == ';')
        && !domain.contains('@')
}

pub fn email(field: &str, s: &str) -> Result<String, ApiError> {
    let v = s.trim().to_lowercase();
    if is_email(&v) {
        Ok(v)
    } else {
        Err(ApiError::bad_request(format!(
            "Datos inválidos — {field}: correo no válido"
        )))
    }
}

pub fn emails(field: &str, list: &[String]) -> Result<Vec<String>, ApiError> {
    list.iter()
        .map(|e| {
            let v = e.trim();
            if is_email(v) {
                Ok(v.to_string())
            } else {
                Err(ApiError::bad_request(format!(
                    "Datos inválidos — {field}: correo no válido ({v})"
                )))
            }
        })
        .collect()
}

pub fn len(field: &str, s: &str, min: usize, max: usize) -> Result<(), ApiError> {
    let n = s.chars().count();
    if n < min {
        return Err(ApiError::bad_request(format!(
            "Datos inválidos — {field}: mínimo {min} caracteres"
        )));
    }
    if n > max {
        return Err(ApiError::bad_request(format!(
            "Datos inválidos — {field}: máximo {max} caracteres"
        )));
    }
    Ok(())
}

pub fn is_domain(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    if s.len() < 3 || s.len() > 253 || !s.contains('.') {
        return false;
    }
    let labels: Vec<&str> = s.split('.').collect();
    let tld = labels.last().copied().unwrap_or_default();
    tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_and_domains() {
        assert!(is_email("ventas@rvmail.local"));
        assert!(!is_email("no-es-correo"));
        assert!(!is_email("a b@x.com"));
        assert!(!is_email("a@x"));
        assert!(is_domain("solucionesrv.net"));
        assert!(!is_domain("-mal.com"));
        assert!(!is_domain("x.c0m"));
    }

    #[test]
    fn spanish_errors() {
        assert_eq!(
            spanish("Failed to deserialize the JSON body into the target type: missing field `password` at line 1 column 16"),
            "password: campo obligatorio"
        );
        assert_eq!(
            spanish("Failed to deserialize the JSON body into the target type: accountId: UUID parsing failed: invalid length at line 1 column 20"),
            "accountId: identificador no válido"
        );
    }
}
