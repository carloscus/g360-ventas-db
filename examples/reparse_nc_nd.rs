use g360_db_ventas::db::writer::{init_pool, insert_ventas, dedup_ventas};
use g360_db_ventas::processor::parser::parse_export_csv_with_cross;
use g360_db_ventas::config::load_dotenv;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

/// Re-parsea todos los CSVs mensuales de raw/ usando el parser nuevo con NC/ND cross-month.
/// No descarga nada — solo re-procesa archivos existentes en disco.
/// Seguro: DELETE+INSERT por mes_ref (idempotente).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .init();
    load_dotenv();

    let raw = g360_db_ventas::config::raw_dir();
    let pool = init_pool().await?;

    // Contar registros antes
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool).await?;
    println!("registros antes: {before}");

    // Listar CSVs mensuales (no diarios)
    let mut csv_files: Vec<PathBuf> = std::fs::read_dir(&raw)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("ventas_")
                && name.ends_with(".csv")
                && !name.contains("-p")
                && name.len() == "ventas_YYYY-MM.csv".len() // solo mensuales
        })
        .collect();
    csv_files.sort();
    println!("CSVs a re-parsear: {}", csv_files.len());

    let mut total_antes = 0usize;
    let mut total_despues = 0usize;

    for csv in &csv_files {
        let label = csv.file_stem().unwrap_or_default().to_string_lossy()
            .replace("ventas_", "");
        match parse_export_csv_with_cross(csv, &pool).await {
            Ok(ventas) => {
                let n = insert_ventas(&pool, &ventas).await?;
                println!("  {label}: {n} filas insertadas");
                total_despues += n;
            }
            Err(e) => {
                eprintln!("  {label}: ERROR {e}");
            }
        }
    }

    // Dedup final
    let dupes = dedup_ventas(&pool).await?;
    println!("duplicados removidos: {dupes}");

    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool).await?;
    println!("registros despues: {after} (antes: {before}, delta: {:+})", after - before);
    println!("re-parse completado: {} meses procesados", csv_files.len());
    Ok(())
}
