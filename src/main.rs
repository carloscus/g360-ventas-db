use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

    match cmd {
        "status" => {
            g360_db_ventas::config::load_dotenv();
            let d = g360_db_ventas::config::data_dir();
            println!("Data dir: {}", d.display());
            let db = g360_db_ventas::config::db_path();
            if db.exists() {
                let p = g360_db_ventas::db::writer::init_pool().await?;
                let t = g360_db_ventas::db::writer::count_ventas(&p).await?;
                println!("Database: {} records", t);
                match g360_db_ventas::db::writer::check_contract_version(&p).await {
                    Ok(_) => println!(
                        "Contract version OK (v{})",
                        g360_db_ventas::db::schema::CONTRACTO_VERSION
                    ),
                    Err(e) => println!("Contract version mismatch: {}", e),
                }
            } else {
                println!("No DB found at {}", db.display());
            }
        }
        _ => {
            println!("g360-db-ventas v{} - Motor de ventas CIPSA", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Comandos:");
            println!("  status       # Estado de la BD local (récords + versión de contrato)");
            println!();
            println!("Bins especializados (cada uno con su --help):");
            println!("  upload           # Reconciliación Supabase mes por mes");
            println!("  capture_daily    # Captura incremental del mes en curso");
            println!("  recapture        # Re-captura un rango de fechas");
            println!("  reparse          # Re-procesa CSVs locales de raw/ a SQLite");
            println!("  importxls        # Importa un XLS mensual manual");
            println!("  export           # Exporta la BD completa");
            println!("  build_initial_cartucho  # Genera cartucho .db + manifest");
            println!("  compare_checksums       # Verifica un cartucho offline");
            println!("  query / chk_fechas / normaliza_fechas / agregar_indices_*");
        }
    }

    Ok(())
}
