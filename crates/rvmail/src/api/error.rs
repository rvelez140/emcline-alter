//! Errores HTTP con el mismo JSON que NestJS (`{ message, error, statusCode }`),
//! para que la web y los clientes existentes no cambien. Mensajes en español,
//! sin trazas internas (regla 9): el detalle técnico solo va al log.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, m)
    }
    pub fn bad_gateway(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, m)
    }
    pub fn too_many(m: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, m)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error = self.status.canonical_reason().unwrap_or("Error");
        (
            self.status,
            Json(json!({ "message": self.message, "error": error, "statusCode": self.status.as_u16() })),
        )
            .into_response()
    }
}

/// Cualquier error inesperado → 500 genérico; el detalle va al log.
macro_rules! internal_from {
    ($($t:ty),*) => {$(
        impl From<$t> for ApiError {
            fn from(e: $t) -> Self {
                tracing::error!(err = %e, "error interno");
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Error interno del servidor")
            }
        }
    )*};
}
internal_from!(
    sqlx::Error,
    redis::RedisError,
    serde_json::Error,
    rvmail_core::crypto::CryptoError
);

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(err = %format!("{e:#}"), "error interno");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Error interno del servidor",
        )
    }
}
