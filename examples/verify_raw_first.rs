// Verificacion raw-first contra la BD local real (solo lectura, sin red).
// Uso: cargo run --example verify_raw_first
use g360_db_ventas::config::with_line_filter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    g360_db_ventas::config::load_dotenv();
    let pool = g360_db_ventas::db::writer::init_pool().await?;

    let raw: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await?;
    let lf = with_line_filter("");
    let elegibles: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM ventas {}", lf))
        .fetch_one(&pool)
        .await?;

    println!("== RAW-FIRST sobre BD local real ==");
    println!("filas raw locales : {raw}");
    println!("elegibles Supabase: {elegibles}");
    println!("excluidas         : {} ({:.2}%)", raw - elegibles, 100.0 * (raw - elegibles) as f64 / raw.max(1) as f64);

    // Desglose de excluidas por causa
    let no_linea: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ventas v WHERE NOT (substr(id_linea, -2) IN (SELECT substr(value, -2) FROM json_each(?))) AND NOT (tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%')"
    )
    .bind(serde_json::to_string(&g360_db_ventas::config::effective_allowed_lines())?)
    .fetch_one(&pool)
    .await?;
    let nc_nd: i64 = raw - elegibles - no_linea;
    println!("  - facturas/otros con linea no permitida: {no_linea}");
    println!("  - NC/ND no habilitadas (factura ref sin linea permitida o inexistente): {nc_nd}");

    // Coherencia: dry-run oficial debe dar lo mismo que el WHERE del filtro
    let dry = g360_db_ventas::processor::uploader::dry_run_upload(&pool, 0, None).await?;
    println!("dry_run_upload(0, None): {} filas (debe ser {elegibles})", dry.rows_to_upload);
    if dry.rows_to_upload as i64 != elegibles {
        anyhow::bail!("DESCUADRE: dry-run {} != filtro {elegibles}", dry.rows_to_upload);
    }
    println!("OK: dry-run y filtro coinciden");
    Ok(())
}
