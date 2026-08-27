use g360_db_ventas::db::writer::{init_pool, insert_ventas};
use g360_db_ventas::processor::parser::parse_export_csv;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let raw = g360_db_ventas::config::raw_dir();
    let csv = raw.join("ventas_2021-01.csv");
    println!("csv: {} exists={} size={}", csv.display(), csv.exists(), std::fs::metadata(&csv).map(|m| m.len()).unwrap_or(0));
    let ventas = parse_export_csv(&csv)?;
    println!("parse_export: {} filas filtradas (de 15497 crudas)", ventas.len());
    let pool = init_pool().await?;
    let n = insert_ventas(&pool, &ventas).await?;
    println!("insert_ventas: {} filas insertadas", n);
    // dedup
    let d = g360_db_ventas::db::writer::dedup_ventas(&pool).await?;
    println!("dedup removidas: {}", d);
    // verificar failed file cleanup
    let failed = raw.join("failed_2021-01.json");
    if n > 0 {
        let _ = std::fs::remove_file(&failed);
        println!("failed_2021-01.json eliminado (si existia)");
    }
    // contar total DB
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await?;
    println!("total ventas en DB: {}", cnt.0);
    let cnt01: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas WHERE mes_ref='2021-01'").fetch_one(&pool).await?;
    println!("ventas 2021-01 en DB: {}", cnt01.0);
    Ok(())
}
