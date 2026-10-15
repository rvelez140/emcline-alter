//! API HTTP de RV Mail (`rvmail api`): REST + SSE + MCP, con los mismos
//! endpoints que la versión NestJS. También sirve la web (PWA) compilada.

mod accounts;
mod auth;
mod clients;
mod discover;
mod error;
mod events;
mod health;
mod mail;
mod mail_jmap;
mod mailserver;
mod mcp;
pub mod migrate;
mod sso;
mod stalwart;
mod state;
mod tokens;
mod validate;

use anyhow::{Context, Result};
use axum::http::{header, HeaderValue, Request};
use axum::Router;
use redis::aio::ConnectionManager;
use rvmail_core::config::{ApiConfig, CoreConfig};
use state::{AppState, Shared};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::info;

pub fn router(st: Shared) -> Router {
    let api = Router::new()
        .merge(health::routes())
        .merge(auth::routes())
        .merge(accounts::routes())
        .merge(mail::routes())
        .merge(events::routes())
        .merge(tokens::routes())
        .merge(mcp::routes())
        .merge(mailserver::routes())
        .merge(sso::routes())
        .merge(discover::routes())
        .fallback(|| async { error::ApiError::not_found("Ruta no encontrada") })
        .layer(axum::extract::DefaultBodyLimit::max(30 * 1024 * 1024));

    let mut app = Router::new().nest("/api", api).with_state(st.clone());

    if !st.cfg.web_dir.is_empty() {
        app = app.fallback_service(web_service(&st.cfg.web_dir));
    }
    // El predicado por defecto no comprime SSE, gRPC ni imágenes
    with_security_headers(app.layer(CompressionLayer::new().gzip(true).zstd(true)))
        .layer(tower_http::trace::TraceLayer::new_for_http().make_span_with(|r: &Request<_>| {
            tracing::debug_span!("http", method = %r.method(), path = %r.uri().path())
        }))
}

/// Web compilada: recursos con hash con caché larga; sw/manifest sin caché; SPA → index.html.
fn web_service(dir: &str) -> Router {
    let index = format!("{dir}/index.html");
    let no_cache = SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    let immutable = SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Router::new()
        .nest_service(
            "/assets",
            tower::ServiceBuilder::new()
                .layer(immutable)
                .service(ServeDir::new(format!("{dir}/assets"))),
        )
        .fallback_service(
            tower::ServiceBuilder::new()
                .layer(no_cache)
                .service(ServeDir::new(dir).fallback(ServeFile::new(index))),
        )
}

/// Cabeceras de seguridad (las mismas que daba nginx).
fn with_security_headers(app: Router) -> Router {
    const HEADERS: [(&str, &str); 5] = [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        ("x-frame-options", "SAMEORIGIN"),
        ("permissions-policy", "camera=(), microphone=(), geolocation=()"),
        (
            "content-security-policy",
            "default-src 'self'; img-src 'self' data: blob: https:; style-src 'self' 'unsafe-inline'; \
             script-src 'self'; connect-src 'self'; frame-src 'self' blob: data: about:; worker-src 'self'; \
             manifest-src 'self'; base-uri 'self'; form-action 'self' https://accounts.google.com \
             https://login.microsoftonline.com https://api.login.yahoo.com",
        ),
    ];
    HEADERS.iter().fold(app, |app, (name, value)| {
        app.layer(SetResponseHeaderLayer::if_not_present(
            header::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        ))
    })
}

pub async fn run() -> Result<()> {
    let core = CoreConfig::from_env()?;
    let cfg = ApiConfig::from_env()?;
    let db = rvmail_core::db::connect(&core.database_url, cfg.db_pool_size).await?;
    if cfg.run_migrations {
        info!("aplicando migraciones…");
        migrate::run(&db).await?;
    }
    let redis_client = redis::Client::open(core.redis_url.as_str())?;
    let redis = ConnectionManager::new(redis_client.clone())
        .await
        .context("No se pudo conectar a Redis")?;
    let (events, _) = broadcast::channel(1024);
    tokio::spawn(events::pump(redis_client, events.clone()));

    let mailserver = mailserver::MailServerService::new(cfg.mailserver.clone());
    let port = cfg.port;
    let st: Shared = Arc::new(AppState {
        db,
        redis,
        core,
        cfg,
        mailserver,
        events,
        http: crate::net::tls::http_client(false),
    });
    let app = router(st);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("API escuchando en :{port}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

async fn shutdown() {
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("señal");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    info!("cerrando API");
    // Las conexiones SSE son largas: no esperar indefinidamente
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        std::process::exit(0);
    });
}
