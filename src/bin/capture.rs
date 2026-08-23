use anyhow::Result;
use chrono::NaiveDate;
use headless_chrome::Browser;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!("g360-db-ventas - Single month capture (XLS download)");

    let browser = Browser::new(
        headless_chrome::LaunchOptionsBuilder::default()
            .headless(true)
            .build()
            .map_err(|e| anyhow::anyhow!("Launch: {}", e))?,
    )
    .map_err(|e| anyhow::anyhow!("Browser: {}", e))?;

    let out = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&out)?;

    let ranges = g360_db_ventas::browser::captor::generate_month_ranges(1);
    let m = &ranges[0];
    info!("Capture {} ({} to {})", m.label, m.start, m.end);

    match g360_db_ventas::browser::captor::capture_month(&browser, m, &out).await {
        Ok(p) => info!("OK: {}", p.display()),
        Err(e) => error!("FAIL: {}", e),
    }
    Ok(())
}
