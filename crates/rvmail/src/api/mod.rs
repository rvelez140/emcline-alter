//! API HTTP de RV Mail (`rvmail api`): REST + SSE + MCP, con los mismos
//! endpoints que la versión NestJS. También sirve la web (PWA) compilada.

mod accounts;
mod app_auth;
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
        .merge(app_auth::routes())
        .merge(accounts::routes())
        .merge(mail::routes())
        .merge(events::routes())
        .merge(tokens::routes())
        .merge(mcp::routes())
        .merge(mailserver::routes())
        .merge(sso::routes())
        .merge(discover::routes())
        .fallback(|| async { error::ApiError::not_found("Ruta no encontrada") })
        .layer(axum::extract::DefaultBodyLimit::max(30 * 1024 * 1024))
        .layer(cors(&st.cfg.cors_origins));

    let mut app = Router::new()
        .nest("/api", api)
        .route(
            "/.well-known/rvmail",
            axum::routing::get(well_known).layer(cors_any()),
        )
        .with_state(st.clone());

    if !st.cfg.web_dir.is_empty() {
        app = app.fallback_service(web_service(&st.cfg.web_dir));
    }
    // El predicado por defecto no comprime SSE, gRPC ni imágenes
    with_security_headers(app.layer(CompressionLayer::new().gzip(true).zstd(true)))
        .layer(tower_http::trace::TraceLayer::new_for_http().make_span_with(|r: &Request<_>| {
            tracing::debug_span!("http", method = %r.method(), path = %r.uri().path())
        }))
}

/// CORS para apps con interfaz local (Tauri) e integraciones: solo con Bearer,
/// sin cookies (`allow_credentials` desactivado), así la sesión web no se expone.
fn cors(origins: &[String]) -> tower_http::cors::CorsLayer {
    use axum::http::Method;
    let list: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    tower_http::cors::CorsLayer::new()
        .allow_origin(list)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT])
        .max_age(Duration::from_secs(3600))
}

fn cors_any() -> tower_http::cors::CorsLayer {
    tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([axum::http::Method::GET])
}

/// Descubrimiento del servidor para las apps: con solo el dominio saben qué ofrece.
async fn well_known(
    axum::extract::State(st): axum::extract::State<Shared>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "service": "rvmail",
        "name": "RV Mail",
        "version": st.cfg.version,
        "api": "/api",
        "auth": {
            "password": true,
            "mailbox": st.mailserver.enabled(),
            "signup": st.cfg.allow_signup,
            "sso": sso::enabled_providers(&st),
            "token": "/api/auth/token",
            "refresh": "/api/auth/token/refresh",
            "ssoStart": "/api/auth/sso/{provider}/start?app_redirect={redirect}&app_challenge={challenge}",
            "ssoExchange": "/api/auth/token/exchange",
            "appRedirectSchemes": ["rvmail"],
        },
        "events": "/api/events",
        "mcp": "/api/mcp",
        "features": {
            "jmapAccounts": true,
            "oauthAccounts": !sso::enabled_providers(&st).is_empty(),
            "mailServer": st.mailserver.enabled(),
        },
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
             script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; frame-src 'self' blob: data: about:; worker-src 'self'; \
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
