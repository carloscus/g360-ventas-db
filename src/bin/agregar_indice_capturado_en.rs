// Agrega idx_venta_capturado_en a una DB existente, sin reconstruir la tabla.
//
// Por que hace falta: `MAX(capturado_en)` es la consulta de frescura del snapshot
// que pega el /api/status del API y el calculo de desfase del cliente. Sin indice
// SQLite planifica SEARCH sobre 2.8M de filas: 47s con el driver C de Python y
// 113s con el driver puro Go del API (2.4x de diferencia entre implementaciones
// sobre la MISMA query). Con el indice pasa a COVERING INDEX y baja a 0.00s.
//
// Idempotente: si el indice ya esta, no hace nada.
//
// USO (con la app cerrada, por el lock de escritura):
//     cargo run --release --bin agregar_indice_capturado_en            # informa
//     cargo run --release --bin agregar_indice_capturado_en --aplicar  # crea
//     cargo run --release --bin agregar_indice_capturado_en --db RUTA  # otra base
use anyhow::{Context, Result, bail};
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const INDICE: &str = "idx_venta_capturado_en";
const COLUMNA: &str = "capturado_en";

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

    let existe: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?",
    )
    .bind(INDICE)
    .fetch_one(&pool)
    .await?;
    if existe > 0 {
        info!("{INDICE} ya existe: nada que hacer.");
        return Ok(());
    }

    let tiene_col: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('ventas') WHERE name=?")
        .bind(COLUMNA)
        .fetch_one(&pool)
        .await?;
    if tiene_col == 0 {
        info!("la tabla `ventas` no tiene {COLUMNA}: nada que hacer.");
        return Ok(());
    }

    // Se mide ANTES: es el numero que justifica la herramienta. Con el indice
    // este query baja a milisegundos y el "antes" del proximo run ya no aplica.
    let t0 = Instant::now();
    let antes: Option<String> =
        sqlx::query_scalar(&format!("SELECT MAX({COLUMNA}) FROM ventas"))
            .fetch_one(&pool)
            .await?;
    let elapsed_antes = t0.elapsed();
    info!(
        "MAX({COLUMNA}) antes: {antes:?}   ({:.1}s)",
        elapsed_antes.as_secs_f64()
    );

    if !aplicar {
        info!("\nsolo informe. Para escribir: --aplicar");
        return Ok(());
    }

    let t1 = Instant::now();
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS {INDICE} ON ventas({COLUMNA})"
    ))
    .execute(&pool)
    .await
    .with_context(|| format!("creando {INDICE}"))?;
    info!(
        "  indice creado en {:.1}s",
        t1.elapsed().as_secs_f64()
    );

    let t2 = Instant::now();
    let despues: Option<String> =
        sqlx::query_scalar(&format!("SELECT MAX({COLUMNA}) FROM ventas"))
            .fetch_one(&pool)
            .await?;
    let elapsed_despues = t2.elapsed();
    info!(
        "MAX({COLUMNA}) despues: {despues:?}   ({:.3}s)",
        elapsed_despues.as_secs_f64()
    );
    if despues != antes {
        bail!("el valor de MAX cambio: {antes:?} -> {despues:?}. No deberia haber pasado.");
    }

    // `EXPLAIN QUERY PLAN` devuelve 4 columnas (id, parent, notused, detail) y la
    // 4a es INTEGER en el protocolo de sqlx, asi que se lee como texto con
    // `cast` en SQL en vez de mapear a String.
    let plan: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(detail AS TEXT) FROM pragma_explain_query_plan('SELECT MAX(capturado_en) FROM ventas')",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();
    for p in &plan {
        info!("  plan: {p}");
    }
    if !plan.iter().any(|p| p.contains(INDICE)) {
        bail!("el plan no usa {INDICE}: el indice no esta sirviendo.");
    }

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='ventas' AND sql IS NOT NULL",
    )
    .fetch_one(&pool)
    .await?;
    info!("  indices en ventas ahora: {n}");
    Ok(())
}