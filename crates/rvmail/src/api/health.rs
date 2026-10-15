//! Salud del servicio: BD y Redis.

use super::state::Shared;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn routes() -> Router<Shared> {
    Router::new().route("/health", get(health))
}

async fn health(State(st): State<Shared>) -> (StatusCode, Json<Value>) {
    let db = sqlx::query("select 1").execute(&st.db).await;
    let mut r = st.redis.clone();
    let redis: Result<String, _> = redis::cmd("PING").query_async(&mut r).await;
    if let (Ok(_), Ok(_)) = (&db, &redis) {
        return (
            StatusCode::OK,
            Json(json!({ "status": "ok", "version": st.cfg.version })),
        );
    }
    // El detalle va al log; al cliente no se le exponen datos internos
    tracing::error!(db = ?db.err(), redis = ?redis.err(), "healthcheck falló");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "status": "error" })),
    )
}
