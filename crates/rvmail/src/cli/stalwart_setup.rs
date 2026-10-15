//! Configuración inicial de Stalwart sin pasar por su asistente web.
//!
//! En el primer arranque Stalwart queda en "modo bootstrap" y acepta al
//! administrador de recuperación (`STALWART_RECOVERY_ADMIN=admin:<clave>`).
//! `x:Bootstrap/set` crea la configuración y devuelve el administrador
//! permanente (`admin@<dominio>` + clave), que se guarda en `.env`.

use super::envfile::EnvFile;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

pub enum Outcome {
    /// Ya estaba configurado y las credenciales de `.env` funcionan.
    AlreadyDone,
    /// Bootstrap hecho: nuevo administrador guardado en `.env`.
    Bootstrapped { user: String },
}

fn base(env: &EnvFile) -> String {
    format!(
        "http://127.0.0.1:{}",
        env.get_or("STALWART_HTTP_PORT", "8180")
    )
}

async fn call(
    http: &reqwest::Client,
    url: &str,
    user: &str,
    pass: &str,
    calls: Value,
) -> Result<reqwest::Response> {
    Ok(http
        .post(format!("{url}/jmap/"))
        .basic_auth(user, Some(pass))
        .json(&json!({ "using": ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"], "methodCalls": calls }))
        .send()
        .await?)
}

/// ¿Las credenciales de administrador de `.env` ya funcionan?
pub async fn admin_works(env: &EnvFile) -> bool {
    let (Some(user), Some(pass)) = (
        env.value("STALWART_ADMIN_USER"),
        env.value("STALWART_ADMIN_PASSWORD"),
    ) else {
        return false;
    };
    let http = crate::net::tls::http_client(false);
    let calls = json!([["x:Domain/query", { "filter": {} }, "c1"]]);
    match call(&http, &base(env), &user, &pass, calls).await {
        Ok(r) if r.status().is_success() => r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| {
                v["methodResponses"][0][0]
                    .as_str()
                    .map(|s| s == "x:Domain/query")
            })
            .unwrap_or(false),
        _ => false,
    }
}

async fn wait_up(http: &reqwest::Client, url: &str, secs: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if http
            .get(format!("{url}/.well-known/jmap"))
            .send()
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

/// Hace el bootstrap si hace falta. `domain` es el dominio de correo principal.
pub async fn ensure(env: &mut EnvFile, mail_host: &str, domain: &str) -> Result<Outcome> {
    let http = crate::net::tls::http_client(false);
    let url = base(env);
    if !wait_up(&http, &url, 120).await {
        bail!("Stalwart no responde en {url}. Revisa: docker compose logs stalwart");
    }
    if admin_works(env).await {
        return Ok(Outcome::AlreadyDone);
    }
    let Some(recovery) = env.value("STALWART_RECOVERY_PASSWORD") else {
        bail!(
            "Falta STALWART_RECOVERY_PASSWORD en .env para la configuración inicial de Stalwart."
        );
    };
    let calls = json!([[
        "x:Bootstrap/set",
        { "update": { "singleton": {
            "serverHostname": mail_host,
            "defaultDomain": domain,
            // 80/443 son de Caddy o de aaPanel/nginx: Stalwart no puede validar ACME por
            // HTTP. El certificado del correo se configura después (DNS-01 en su panel).
            "requestTlsCertificate": false,
            "generateDkimKeys": true,
        }}},
        "c1"
    ]]);
    let res = call(&http, &url, "admin", &recovery, calls)
        .await
        .context("No se pudo hablar con Stalwart")?;
    if res.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!(
            "Stalwart rechazó al administrador de recuperación. Si ya estaba configurado, pon su \
             administrador en STALWART_ADMIN_USER / STALWART_ADMIN_PASSWORD (rvmail config)."
        );
    }
    let body: Value = res
        .json()
        .await
        .context("Respuesta no válida de Stalwart")?;
    let reply = &body["methodResponses"][0][1];
    let created = &reply["updated"]["singleton"];
    let (Some(user), Some(secret)) = (created["username"].as_str(), created["secret"].as_str())
    else {
        let why = reply["notUpdated"]["singleton"]["description"]
            .as_str()
            .or(reply["description"].as_str())
            .unwrap_or("respuesta inesperada");
        bail!("Stalwart no aceptó la configuración inicial: {why}");
    };
    env.set("STALWART_ADMIN_USER", user);
    env.set("STALWART_ADMIN_PASSWORD", secret);
    env.save()?;
    Ok(Outcome::Bootstrapped {
        user: user.to_string(),
    })
}
