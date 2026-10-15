//! Migraciones SQL embebidas en el binario (sqlx). Compatibilidad: si la base
//! viene de la versión Node (tabla `drizzle.__drizzle_migrations`), las
//! migraciones que Drizzle ya aplicó se registran como hechas sin reejecutarse.

use anyhow::{Context, Result};
use sqlx::migrate::{Migrate, Migrator};
use sqlx::PgPool;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn run(db: &PgPool) -> Result<()> {
    baseline_from_drizzle(db).await?;
    MIGRATOR
        .run(db)
        .await
        .context("No se pudieron aplicar las migraciones")?;
    Ok(())
}

async fn baseline_from_drizzle(db: &PgPool) -> Result<()> {
    let drizzle: Option<String> =
        sqlx::query_scalar("select to_regclass('drizzle.__drizzle_migrations')::text")
            .fetch_one(db)
            .await?;
    if drizzle.is_none() {
        return Ok(());
    }
    let applied: i64 = sqlx::query_scalar("select count(*) from drizzle.__drizzle_migrations")
        .fetch_one(db)
        .await?;
    let mut conn = db.acquire().await?;
    conn.ensure_migrations_table("_sqlx_migrations").await?;
    let already: i64 = sqlx::query_scalar("select count(*) from _sqlx_migrations")
        .fetch_one(&mut *conn)
        .await?;
    if already > 0 {
        return Ok(());
    }
    // Las migraciones de crates/rvmail/migrations son copia 1:1 de packages/db/drizzle, en el mismo orden
    for m in MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .take(applied as usize)
    {
        sqlx::query(
            "insert into _sqlx_migrations (version, description, success, checksum, execution_time) values ($1, $2, true, $3, 0)",
        )
        .bind(m.version)
        .bind(&*m.description)
        .bind(&*m.checksum)
        .execute(&mut *conn)
        .await?;
        tracing::info!(
            version = m.version,
            "migración de Drizzle registrada como aplicada"
        );
    }
    Ok(())
}

/// Espera (hasta 5 min) a que estén aplicadas todas las migraciones que conoce este binario.
pub async fn wait_until_applied(db: &PgPool) -> Result<()> {
    let latest = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    for attempt in 0..150 {
        let ok: bool = sqlx::query_scalar(
            "select coalesce(to_regclass('_sqlx_migrations') is not null
                and exists(select 1 from _sqlx_migrations where version = $1 and success), false)",
        )
        .bind(latest)
        .fetch_one(db)
        .await
        .unwrap_or(false);
        if ok {
            return Ok(());
        }
        if attempt == 0 {
            tracing::info!("esperando a que la API aplique las migraciones…");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    anyhow::bail!("la base de datos no tiene el esquema esperado (¿la API no arrancó?)")
}
