// Agrega a una DB existente los indices que el proyecto define pero una base
// vieja puede no tener. Idempotente: si ya estan, no hace nada.
//
// Por que hace falta: `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`
// de `init_pool` solo aplican a bases nuevas. Una DB instalada antes del indice
// sigue sin el, y las consultas que dependen de el pasan de milisegundos a
// minutos sin dar ningun error.
//
// Los tres indices:
//
//   idx_venta_capturado_en  -> MAX(capturado_en), la frescura del snapshot que
//       pegan /api/status y el cliente. Sin indice: 47s (driver C) / 113s (driver
//       puro Go del API). Con indice: 0.00s.
//
//   idx_venta_mes_checksum  -> COVERING del GROUP BY de checksums mensuales.
//       Sin indice: `SCAN ventas USING INDEX idx_venta_mes` que todavia visita la
//       tabla por fila. Con indice: 36.23s -> 0.92s (medido sobre 2.8M filas).
//
// USO (con la app cerrada, por el lock de escritura):
//     cargo run --release --bin agregar_indices_faltantes            # informa
//     cargo run --release --bin agregar_indices_faltantes --aplicar  # crea
//     cargo run --release --bin agregar_indices_faltantes --db RUTA  # otra base
use anyhow::{Context, Result, bail};
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// (nombre, sql, query que lo demuestra y su tiempo esperado sin el)
const INDICES: &[(&str, &str, &str)] = &[
    (
        "idx_venta_capturado_en",
        "CREATE INDEX IF NOT EXISTS idx_venta_capturado_en ON ventas(capturado_en)",
        "SELECT MAX(capturado_en) FROM ventas",
    ),
    (
        "idx_venta_mes_checksum",
        "CREATE INDEX IF NOT EXISTS idx_venta_mes_checksum \
         ON ventas(mes_ref, soles, COALESCE(dolares,0), cantidad)",
        "SELECT mes_ref, COUNT(*), ROUND(SUM(soles),2), ROUND(SUM(COALESCE(dolares,0)),2), \
                ROUND(SUM(cantidad),2) FROM ventas GROUP BY mes_ref",
    ),
];

async fn existe(pool: &SqlitePool, nombre: &str) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?")
        .bind(nombre)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

async fn medir(pool: &SqlitePool, sql: &str) -> f64 {
    let t = Instant::now();
    let _ = sqlx::query(sql).fetch_all(pool).await;
    t.elapsed().as_secs_f64()
}

/// `EXPLAIN QUERY PLAN` devuelve 4 columnas y la 4a viene declarada INTEGER, asi
/// que hay que castearla en SQL para poder leerla como texto.
async fn plan(pool: &SqlitePool, sql: &str) -> Vec<String> {
    sqlx::query_scalar(&format!(
        "SELECT CAST(detail AS TEXT) FROM pragma_explain_query_plan('{sql}')"
    ))
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let aplicar = args.iter().any(|a| a == "--aplicar");
    let db_override = args
        .iter()
        .position(|a| a == "--db")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let pool: SqlitePool = match &db_override {
        Some(ruta) => {
            let opts = SqliteConnectOptions::from_str(&format!("sqlite://{ruta}?mode=rwc"))
                .map_err(|e| anyhow::anyhow!("--db invalido ({ruta}): {e}"))?
                .create_if_missing(false)
                .busy_timeout(Duration::from_secs(30));
            SqlitePool::connect_with(opts).await?
        }
        None => g360_db_ventas::db::writer::init_pool().await?,
    };
    let ruta: String =
        sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|_| "(desconocida)".into());
    info!("DB: {ruta}");
    if db_override.is_some() {
        warn!("--db: base indicada a mano, NO la de la app");
    }

    let falta_columna: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('ventas')")
            .fetch_one(&pool)
            .await?;
    if falta_columna == 0 {
        bail!("la DB no tiene tabla `ventas`");
    }

    let mut faltantes = Vec::new();
    for (nombre, sql, prueba) in INDICES {
        if existe(&pool, nombre).await? {
            let s = medir(&pool, prueba).await;
            info!("  {nombre}: ya existe   ({prueba} en {s:.3}s)");
        } else {
            let s = medir(&pool, prueba).await;
            info!("  {nombre}: FALTA       ({prueba} en {s:.2}s <- lo que cuesta)");
            faltantes.push((nombre, sql, prueba));
        }
    }

    if faltantes.is_empty() {
        info!("\ntodos los indices estan. nada que hacer.");
        return Ok(());
    }
    if !aplicar {
        info!("\n{} indice(s) faltante(s). Para escribir: --aplicar", faltantes.len());
        return Ok(());
    }

    for (nombre, sql, prueba) in &faltantes {
        info!("\ncreando {nombre}...");
        let t = Instant::now();
        sqlx::query(sql).execute(&pool).await.with_context(|| {
            format!("creando {nombre} (cerrar la app: hay una DB abierta)")
        })?;
        info!("  creado en {:.1}s", t.elapsed().as_secs_f64());

        let s = medir(&pool, prueba).await;
        info!("  ({prueba} ahora en {s:.3}s)");

        for p in plan(&pool, prueba).await {
            info!("    plan: {p}");
        }
    }

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='ventas' AND sql IS NOT NULL",
    )
    .fetch_one(&pool)
    .await?;
    info!("\nindices en ventas ahora: {n}");
    Ok(())
}