// Re-parsea los CSV mensuales de raw/ a SQLite (reemplazo idempotente por mes).
// No re-descarga: usa los archivos ya presentes en raw/.
// Uso: cargo run --release --bin reparse
use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();

    let raw = g360_db_ventas::config::raw_dir();
    let pool = g360_db_ventas::db::writer::init_pool().await?;

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&raw) {
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let monthly = name.len() == "ventas_YYYY-MM.csv".len();
            if name.starts_with("ventas_") && name.ends_with(".csv") && !name.contains("-p") && monthly {
                files.push(p);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        error!("No hay CSVs mensuales en {} para reprocesar", raw.display());
        return Ok(());
    }

    let total_files = files.len();
    info!("Reparse: {} archivos mensuales", total_files);
    let mut total = 0usize;
    for (i, csv) in files.iter().enumerate() {
        match g360_db_ventas::processor::parser::parse_export_csv_with_cross(csv, &pool).await {
            Ok(ventas) if !ventas.is_empty() => {
                match g360_db_ventas::db::writer::insert_ventas(&pool, &ventas, false).await {
                    Ok(n) => total += n,
                    Err(e) => error!("insert {}: {}", csv.display(), e),
                }
            }
            Ok(_) => {}
            Err(e) => error!("parse {}: {}", csv.display(), e),
        }
        if (i + 1) % 10 == 0 || i + 1 == total_files {
            info!("  {}/{} ({} filas)", i + 1, total_files, total);
        }
    }
    let _ = g360_db_ventas::db::writer::refresh_stats_cache(&pool).await;
    info!("Reparse OK: {} filas", total);
    Ok(())
}
