use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();
    let pool = g360_db_ventas::db::writer::init_pool().await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await?;
    println!("Records: {}", total);
    if total == 0 {
        println!("No data.");
        return Ok(());
    }
    let ms: Vec<(String, i64)> =
        sqlx::query_as("SELECT mes_ref, COUNT(*) FROM ventas GROUP BY mes_ref ORDER BY mes_ref")
            .fetch_all(&pool)
            .await?;
    println!("\nPer month:");
    for (m, c) in &ms {
        println!("  {}: {}", m, c);
    }
    let cs: Vec<(String, String, i64, f64)> = sqlx::query_as("SELECT id_cliente, nom_cliente, COUNT(*), COALESCE(SUM(soles),0.0) FROM ventas WHERE tpo_doc IN ('F01','F03','F07','F08') GROUP BY id_cliente,nom_cliente ORDER BY SUM(soles) DESC LIMIT 20").fetch_all(&pool).await?;
    println!("\nTop 20 clients:");
    for (i, n, d, t) in &cs {
        println!("  {:<8} {:<40} {} docs  S/ {:>12.2}", i, n, d, t);
    }
    let ss: Vec<(String, String, i64, f64)> = sqlx::query_as("SELECT id_articulo, nom_articulo, COUNT(*), COALESCE(SUM(soles),0.0) FROM ventas WHERE tpo_doc IN ('F01','F03','F07','F08') GROUP BY id_articulo,nom_articulo ORDER BY SUM(soles) DESC LIMIT 20").fetch_all(&pool).await?;
    println!("\nTop 20 SKUs:");
    for (i, n, d, t) in &ss {
        let n2 = if n.len() > 35 { &n[..35] } else { n.as_str() };
        println!("  {:<8} {:<35} {} docs  S/ {:>12.2}", i, n2, d, t);
    }
    Ok(())
}
