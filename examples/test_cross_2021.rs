use g360_db_ventas::db::writer::init_pool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    g360_db_ventas::config::load_dotenv();
    let pool = init_pool().await?;
    let csv = g360_db_ventas::config::raw_dir().join("ventas_2021-01.csv");

    // Conteo antes
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref='2021-01'").fetch_one(&pool).await?;
    println!("2021-01 antes: {} regs", before);

    // Parse con cross-mes
    let ventas = g360_db_ventas::processor::parser::parse_export_csv_with_cross(&csv, &pool).await?;
    println!("parse_export_csv_with_cross: {} regs (vs 12927 sin cross)", ventas.len());
    let nc_pending = ventas.iter().filter(|v| v.tpo_doc.contains("NCR") || v.tpo_doc.contains("NDB")).count();
    println!("  de ellos NC/ND: {}", nc_pending);

    // Comparar con parse sin cross (solo en-archivo)
    let ventas_sin = g360_db_ventas::processor::parser::parse_export_csv(&csv)?;
    println!("parse_export_csv (sin cross): {} regs", ventas_sin.len());

    println!("delta cross-mes: +{} regs", ventas.len() as i64 - ventas_sin.len() as i64);

    // Mostrar cuántas NC/ND de las 74 excluidas ahora entrarían
    // Para eso, leer el raw sin filtrar y ver el total
    Ok(())
}
