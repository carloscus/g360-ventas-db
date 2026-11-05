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
        "capture" => {
            println!("capture: Capture desde intranet (mes actual o especificado)");
            println!("Uso: g360-db-ventas capture [--month YYYY-MM]");
            // TODO: implementar lógica de captura
        }
        "batch" => {
            println!("batch: Captura historica masiva (rango de meses)");
            println!("Uso: g360-db-ventas batch --start YYYY-MM --end YYYY-MM");
            // TODO: implementar logica de captura en lote
        }
        "export" => {
            println!("export: Genera cartucho canonico (.db + manifest.json)");
            println!("Uso: g360-db-ventas export [--output ./path/output.db]");
            // TODO: llamar a export_canonical_snapshot
        }
        "verify" => {
            println!("verify: Verifica integridad de un cartucho existente");
            println!("Uso: g360-db-ventas verify --db ./path/cartucho.db");
            // TODO: verificar SHA256, versions, checksums
        }
        "replace" => {
            println!("replace: Reemplaza la BD actual desde un cartucho de respaldo");
            println!("Uso: g360-db-ventas replace --db ./path/cartucho.db");
            // TODO: backup -> replace -> init_pool -> verify
        }
        "status" => {
            println!("status: Muestra estado general de la BD local");
            g360_db_ventas::config::load_dotenv();
            let d = g360_db_ventas::config::data_dir();
            println!("Data dir: {}", d.display());
            let db = g360_db_ventas::config::db_path();
            if db.exists() {
                let p = g360_db_ventas::db::writer::init_pool().await?;
                let t = g360_db_ventas::db::writer::count_ventas(&p).await?;
                println!("Database: {} records", t);
                // Verificar version de contrato
                match g360_db_ventas::db::writer::check_contract_version(&p).await {
                    Ok(_) => println!("✓ Contract version OK (v{})", g360_db_ventas::db::schema::CONTRACTO_VERSION),
                    Err(e) => println!("✗ Contract version mismatch: {}", e),
                }
            } else {
                println!("No DB found at {}", db.display());
            }
        }
        _ => {
            println!("g360-db-ventas v{} - Motor de ventas CIPSA", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Commands:");
            println!("  capture      # Capturar mes actual desde intranet");
            println!("  batch        # Captura historica en lote (--start --end)");
            println!("  export       # Generar cartucho canonico (.db + manifest.json)");
            println!("  verify       # Verificar integridad de un cartucho");
            println!("  replace      # Reemplazar BD actual desde cartucho (backup auto)");
            println!("  status       # Estado general de la BD local");
            println!();
            println!("Options:");
            println!("  --help       # This help");
            println!("  --version    # Version actual");
            println!();
            println!("Examples:");
            println!("  g360-db-ventas status");
            println!("  g360-db-ventas export --output ./cartuchos/base_20261104.db");
            println!("  g360-db-ventas verify --db ./cartuchos/base_20261104.db");
            println!("  g360-db-ventas replace --db ./cartuchos/base_20261104.db");
        }
    }

    Ok(())
}
