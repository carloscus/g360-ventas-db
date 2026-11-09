// Binario para comparar checksums entre dos DBs.
// Uso:
//   cargo run --bin compare-checksums -- --db ruta/db1.db --output out1.json
//   cargo run --bin compare-checksums -- --db ruta/db2.db --output out2.json
// Luego diff de los JSON.
//
// Tambien puede usarse para volcar checksums de una sola DB:
//   cargo run --bin compare-checksums -- --db ruta/db.db
use anyhow::{Context, Result};
use sqlx::{query_as, SqlitePool};
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args: Vec<String> = std::env::args().collect();

    // Parsear argumentos
    let mut db_path: Option<String> = None;
    let mut output_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                i += 1;
                if i < args.len() {
                    db_path = Some(args[i].clone());
                }
            }
            "--output" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }

    let db_path = db_path.context("--db es requerido")?;
    let pool = init_pool(&db_path).await?;

    // Verificar versión de contrato
    let version: Option<String> = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'contracto_version'"
    )
    .fetch_optional(&pool)
    .await?;
    match version {
        Some(v) => info!("Contracto version: v{}", v),
        None => error!("Advertencia: g360_meta no encontrado; la DB puede estar desactualizada"),
    }

    // Volcar checksums mensuales
    let month_checksums: Vec<(String, String, i64, f64)> = query_as(
        "SELECT mes_ref, checksum, total_filas, total_soles \
         FROM mes_checksums ORDER BY mes_ref"
    )
    .fetch_all(&pool)
    .await
    .context("Error al leer mes_checksums")?;

    // Volcar checksums diarios
    let day_checksums: Vec<(String, i64, f64, String)> = query_as(
        "SELECT dia, filas, soles, checksum FROM day_checksums ORDER BY dia"
    )
    .fetch_all(&pool)
    .await
    .context("Error al leer day_checksums")?;

    // Metrics basicas
    let total_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

    let rango: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT MIN(fecha_orig), MAX(fecha_orig) FROM ventas"
    )
    .fetch_one(&pool)
    .await
    .unwrap_or((None, None));

    let output = serde_json::json!({
        "total_rows": total_rows,
        "rango": {
            "desde": rango.0.unwrap_or_default(),
            "hasta": rango.1.unwrap_or_default()
        },
        "mes_checksums": month_checksums.iter().map(|(m, c, f, s)| serde_json::json!({
            "mes_ref": m,
            "checksum": c,
            "filas": f,
            "soles": s
        })).collect::<Vec<_>>(),
        "day_checksums": day_checksums.iter().map(|(d, f, s, c)| serde_json::json!({
            "dia": d,
            "filas": f,
            "soles": s,
            "checksum": c
        })).collect::<Vec<_>>()
    });

    match &output_path {
        Some(path) => {
            std::fs::write(path, serde_json::to_string_pretty(&output)?)
                .context("Error al escribir archivo de salida")?;
            info!("Checksums escritos en: {}", path);
        }
        None => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}

async fn init_pool(path: &str) -> Result<SqlitePool> {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};

    let opts = SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Delete)
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(opts)
        .await
        .context(format!("No se pudo conectar a {}: verifica que la ruta sea correcta", path))?;
    Ok(pool)
}
