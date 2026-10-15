//! SSO (OpenID Connect) y cuentas por OAuth2 — se completa en la fase S.

use super::error::{ApiError, ApiResult};
use super::state::Shared;
use rvmail_core::models::MailAccount;
use serde_json::Value;

/// Proveedores de inicio de sesión configurados (para los botones del login).
pub fn enabled_providers(_st: &Shared) -> Vec<Value> {
    vec![]
}

/// Access token vigente para una cuenta OAuth2 (refresca si hace falta).
pub async fn access_token_for(_st: &Shared, _a: &MailAccount) -> ApiResult<String> {
    Err(ApiError::bad_gateway(
        "Las cuentas OAuth aún no están disponibles",
    ))
}
