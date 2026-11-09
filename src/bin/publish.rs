// Binario para publicar incremental los días con cambios hacia Supabase.
// Uso:
//   publish                     -> publica días desde el último publicado
//   publish --full              -> força publicación de todos los días (resetea estado)
//
// El comando compara checksums diarios locales con los publicados en etl_state;
// solo sube los días que cambiaron. Reporta filas publicadas y actualiza el
// estado incremental.
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
    let mut full = false;
    let mut help = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--full" => full = true,
            "--help" | "-h" => help = true,
            _ => {}
        }
        i += 1;
    }

    if help {
        eprintln!("Usage: publish [--full]");
        eprintln!();
        eprintln!("Publica incremental los días con cambios hacia Supabase.");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --full   Fuerza publicación de todos los días (resetea estado incremental)");
        eprintln!("  --help, -h  Show this help");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  publish              -> publica días nuevos/modificados desde el último sync");
        eprintln!("  publish --full       -> publica todo el historial disponible");
        std::process::exit(0);
    }

    let pool = g360_db_ventas::db::writer::init_pool().await?;

    // Si --full: resetear etl_state para forzar re-publicación completa
    if full {
        info!("Modo FULL: reseteando estado incremental");
        sqlx::query(
            "DELETE FROM etl_state WHERE key LIKE 'checksum_%' OR key = 'ultimo_dia_publicado'",
        )
        .execute(&pool)
        .await?;
    }

    info!("Ejecutando publish_incremental...");
    match g360_db_ventas::db::writer::publish_incremental(&pool).await {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            info!("Publish completado: {}", report);
        }
        Err(e) => {
            error!("Publish falló: {e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}
