// Verificación simple NC/ND 2021 vs facturas
use g360_db_ventas::db::writer::init_pool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .init();
    g360_db_ventas::config::load_dotenv();
    let pool = init_pool().await?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await?;
    println!("total registros: {total}");

    let nc: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ventas WHERE mes_ref LIKE '2021%' AND (tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%')"
    ).fetch_one(&pool).await?;
    println!("NC/ND 2021: {nc}");

    let sin_ref: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ventas WHERE mes_ref LIKE '2021%' AND (tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%') AND (factura_ref_serie = '' OR factura_ref_nro = '')"
    ).fetch_one(&pool).await?;
    println!("sin referencia clara: {sin_ref}");

    let con_ref: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ventas WHERE mes_ref LIKE '2021%' AND (tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%') AND factura_ref_serie != ''"
    ).fetch_one(&pool).await?;
    println!("con referencia: {con_ref}");

    let c2020: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref LIKE '2020%'").fetch_one(&pool).await?;
    println!("registros 2020: {c2020}");

    let c2021: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref LIKE '2021%'").fetch_one(&pool).await?;
    println!("registros 2021: {c2021}");

    // por mes
    println!("\npor mes 2021:");
    let rows: Vec<(String,i64,i64)> = sqlx::query_as(
        "SELECT mes_ref, COUNT(*) as t, SUM(CASE WHEN tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%' THEN 1 ELSE 0 END) as nc FROM ventas WHERE mes_ref LIKE '2021%' GROUP BY mes_ref ORDER BY mes_ref"
    ).fetch_all(&pool).await?;
    for r in rows {
        println!("  {}: total={}, nc/nd={}", r.0, r.1, r.2);
    }

    Ok(())
}
