// Importa un XLS mensual descargado del intranet a la BD local (reemplaza el mes).
// Uso: importxls <YYYY-MM> <ruta.xls>
// Replica el flujo del comando Tauri `import_manual_month`:
//   copia a raw/ventas_<mes>.xls -> xls_to_csv -> parse -> insert_ventas (reemplaza el mes)
use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    g360_db_ventas::config::load_dotenv();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Uso: importxls <YYYY-MM> <ruta.xls>");
        std::process::exit(1);
    }
    let month = args[1].clone();
    let xls = std::path::PathBuf::from(&args[2]);
    if month.len() != 7 || !month.contains('-') {
        error!("Formato mes debe ser YYYY-MM");
        std::process::exit(1);
    }
    if !xls.exists() {
        error!("XLS no existe: {}", xls.display());
        std::process::exit(1);
    }

    // Gate de integridad: SKU guardado como numero (perdida de cero inicial).
    if let Ok(insp) = g360_db_ventas::processor::xls::inspect_sku_column(&xls) {
        if insp.has_numeric_sku() {
            error!("Columna SKU numerica ({} filas): re-exporta el XLS sin re-guardarlo en Excel.", insp.numeric_cells);
            std::process::exit(1);
        }
    }

    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;
    let dest_xls = raw.join(format!("ventas_{}.xls", month));
    let dest_csv = raw.join(format!("ventas_{}.csv", month));
    std::fs::copy(&xls, &dest_xls)?;

    let n_csv = g360_db_ventas::processor::xls::xls_to_csv(&dest_xls, &dest_csv)?;
    info!("xls_to_csv: {} filas -> {}", n_csv, dest_csv.display());

    let (ventas, stats) = g360_db_ventas::processor::parser::parse_export_csv(&dest_csv)?;
    if ventas.is_empty() {
        error!("Parse devolvio 0 filas (¿reporte Estadistica11?)");
        std::process::exit(1);
    }
    info!("parse: {} filas ({} invalidas: {} fechas, {} futuras, {} sin ID)",
        ventas.len(),
        stats.invalid_dates + stats.future_dates + stats.empty_ids,
        stats.invalid_dates, stats.future_dates, stats.empty_ids);

    let pool = g360_db_ventas::db::writer::init_pool().await?;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
        .bind(&month)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas, true).await?;
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
        .bind(&month)
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", month)));
    info!(
        "Mes {}: antes={} insertadas={} despues={}",
        month, before, n, after
    );
    Ok(())
}
