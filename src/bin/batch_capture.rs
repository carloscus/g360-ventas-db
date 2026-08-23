use anyhow::Result;
use headless_chrome::Browser;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

async fn try_capture(
    browser: &Browser,
    month: &g360_db_ventas::browser::captor::MonthRange,
    raw: &std::path::Path,
) -> Result<std::path::PathBuf> {
    g360_db_ventas::browser::captor::capture_month(browser, month, raw).await
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let n = g360_db_ventas::config::MONTHS_BACK;
    info!("g360-db-ventas - Batch capture ({} months)", n);
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;

    let ranges = g360_db_ventas::browser::captor::generate_month_ranges(n);
    for (i, r) in ranges.iter().enumerate() {
        info!("  [{}] {} => {}", i + 1, r.start, r.end);
    }

    let mut ok = Vec::new();
    let mut fail = Vec::new();

    for (idx, m) in ranges.iter().enumerate() {
        info!("=== {}/{}: {} ===", idx + 1, ranges.len(), m.label);

        let mut success = false;
        for attempt in 1..=3 {
            info!("  Attempt {}/3", attempt);

            // Create fresh browser for each attempt
            let browser = match Browser::new(
                headless_chrome::LaunchOptionsBuilder::default()
                    .headless(true)
                    .build()
                    .map_err(|e| anyhow::anyhow!("Launch: {}", e))?,
            ) {
                Ok(b) => b,
                Err(e) => {
                    warn!("  Browser launch failed: {}", e);
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            match try_capture(&browser, m, &raw).await {
                Ok(p) => {
                    info!("OK: {}", p.display());
                    ok.push(m.label.clone());
                    success = true;
                    break;
                }
                Err(e) => {
                    warn!("  Attempt {} failed: {}", attempt, e);
                    sleep(Duration::from_secs(3)).await;
                }
            }
            // Browser dropped here
        }

        if !success {
            fail.push((m.label.clone(), "All 3 attempts failed".to_string()));
        }

        if idx < ranges.len() - 1 {
            let d = g360_db_ventas::config::SLEEP_BETWEEN_MONTHS;
            info!("Sleep {}s...", d);
            sleep(Duration::from_secs(d)).await;
        }
    }

    info!(
        "=== DONE: {}/{} ok, {} failed ===",
        ok.len(),
        ranges.len(),
        fail.len()
    );
    let s = serde_json::json!({"ok": ok, "fail": fail, "total": ranges.len()});
    std::fs::write(raw.join("summary.json"), serde_json::to_string_pretty(&s)?)?;
    Ok(())
}
