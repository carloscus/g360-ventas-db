// Exporta la instantanea canonica (snapshot .db + manifiesto JSON).
// Uso:
//   export                       -> <data>/export/base_canonica_<ts>.db (+ .manifest.json)
//   export <carpeta>             -> <carpeta>/base_canonica_<ts>.db (+ .manifest.json)
//   export <ruta.db>             -> esa ruta exacta (+ <ruta>.manifest.json)
// Util para el flujo USB / carpeta compartida (automatizable).
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
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let default_dir = g360_db_ventas::config::data_dir().join("export");

    let target: std::path::PathBuf = if args.len() >= 2 {
        let arg = std::path::PathBuf::from(&args[1]);
        let looks_like_file = arg
            .extension()
            .map(|e| e.eq_ignore_ascii_case("db"))
            .unwrap_or(false);
        if looks_like_file {
            arg
        } else {
            std::fs::create_dir_all(&arg).ok();
            arg.join(format!("base_canonica_{}.db", ts))
        }
    } else {
        std::fs::create_dir_all(&default_dir).ok();
        default_dir.join(format!("base_canonica_{}.db", ts))
    };

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    info!("Exportando base canonica -> {}", target.display());
    match g360_db_ventas::db::writer::export_canonical_snapshot(&target, None).await {
        Ok(info_json) => {
            println!("{}", serde_json::to_string_pretty(&info_json).unwrap_or_default());
            info!("Listo: comparte el .db + el .manifest.json con la app consumidora");
        }
        Err(e) => {
            error!("Fallo el export: {e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}
