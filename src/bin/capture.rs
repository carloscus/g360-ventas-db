use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use headless_chrome::Browser;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    g360_db_ventas::config::load_dotenv();

    info!("g360-db-ventas - Single month capture (XLS download)");
    let started_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    info!("Iniciado: {}", started_at);

    let browser = Browser::new(
        headless_chrome::LaunchOptionsBuilder::default()
            .headless(true)
            .build()
            .map_err(|e| anyhow::anyhow!("Launch: {}", e))?,
    )
    .map_err(|e| anyhow::anyhow!("Browser: {}", e))?;

    let out = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&out)?;

    // Lock check
    let lock_path = out.join("capture.lock");
    if lock_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&lock_path) {
            info!("Lock activo: {}", content.lines().next().unwrap_or(""));
        }
        std::process::exit(1);
    }
    std::fs::write(&lock_path, format!("pid={}\nstarted={}\n", std::process::id(), started_at))?;

    let ranges = g360_db_ventas::browser::captor::generate_month_ranges(1);
    // Rango util: primer dia del mes actual -> hoy (no el mes pasado completo)
    let today = chrono::Local::now().date_naive();
    let mut m = ranges.into_iter().next().expect("ranges");
    m.start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
    m.end = today;
    info!("Capture {} ({} to {})", m.label, m.start, m.end);

    match g360_db_ventas::browser::captor::capture_month(&browser, &m, &out).await {
        Ok(p) => info!("OK: {}", p.display()),
        Err(e) => error!("FAIL: {}", e),
    }

    let _ = std::fs::remove_file(&lock_path);
    info!("Finalizado: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    Ok(())
}
