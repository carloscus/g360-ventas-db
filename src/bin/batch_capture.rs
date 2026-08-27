use anyhow::Result;
use headless_chrome::Browser;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use g360_db_ventas::capture_state::{CapturePhase, ProgressState, SharedProgress, now_secs};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    g360_db_ventas::config::load_dotenv();

    info!("g360-db-ventas - Batch capture ({} months)", g360_db_ventas::config::MONTHS_BACK);
    let started_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    info!("Iniciado: {}", started_at);

    let shared: SharedProgress = Arc::new(std::sync::Mutex::new(ProgressState::new()));
    let mut s = shared.lock().unwrap();
    s.set_start(&format!("Batch capture {} meses", g360_db_ventas::config::MONTHS_BACK));
    drop(s);

    // Spawn progress watcher
    let shared_w = shared.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(3)).await;
            let st = shared_w.lock().unwrap();
            let elapsed = if let Some(start) = st.started_at {
                now_secs().saturating_sub(start)
            } else { 0 };
            info!(
                "[BATCH-PROGRESS] phase={} progress={:.1}% msg=\"{}\" current=\"{}\" elapsed={:02}:{:02}",
                st.phase, st.progress * 100.0, st.message, st.current_item,
                elapsed / 60, elapsed % 60
            );
        }
    });

    let n = g360_db_ventas::config::MONTHS_BACK;
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;

    // Lock file
    let lock_path = raw.join("capture.lock");
    if lock_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&lock_path) {
            warn!("Lock existente: {}", content.lines().next().unwrap_or(""));
        }
        eprintln!("ERROR: Otro proceso puede estar activo. Elimine capture.lock si es seguro.");
        std::process::exit(1);
    }
    std::fs::write(&lock_path, format!("pid={}\nstarted={}\nn_months={}\n", std::process::id(), started_at, n))?;

    let ranges = g360_db_ventas::browser::captor::generate_month_ranges(n);
    for (i, r) in ranges.iter().enumerate() {
        info!("  [{}] {} => {}", i + 1, r.start, r.end);
    }

    let mut ok = Vec::new();
    let mut fail = Vec::new();
    let total = ranges.len();

    for (idx, m) in ranges.iter().enumerate() {
        let pct = (idx as f32 / total as f32) * 85.0;
        {
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Downloading, format!("Descargando {}", m.label));
            s.set_current(&m.label);
            s.update_progress(pct, format!("[{}/{}] {}", idx + 1, total, m.label));
        }
        info!("=== {}/{}: {} ===", idx + 1, total, m.label);

        let mut success = false;
        for attempt in 1..=3 {
            {
                let mut s = shared.lock().unwrap();
                s.update_progress(pct, format!("  intento {}/3 — {}", attempt, m.label));
                s.set_current(&format!("{} (intento {attempt})", m.label));
            }
            info!("  Attempt {}/3", attempt);

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

            match g360_db_ventas::browser::captor::capture_month(&browser, m, &raw).await {
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
        }

        if !success {
            fail.push((m.label.clone(), "All 3 attempts failed".to_string()));
        }

        if idx < total - 1 {
            let d = g360_db_ventas::config::SLEEP_BETWEEN_MONTHS;
            info!("Sleep {}s...", d);
            sleep(Duration::from_secs(d)).await;
        }
    }

    // Write summary
    let s_json = serde_json::json!({"ok": ok, "fail": fail, "total": total});
    std::fs::write(raw.join("summary.json"), serde_json::to_string_pretty(&s_json)?).ok();

    // Clean lock and finalize
    let _ = std::fs::remove_file(&lock_path);

    {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::Done, "Completado");
        s.update_progress(1.0, &format!("Listo — {}/{} ok, {}/{} fail", ok.len(), total, fail.len(), total));
    }

    info!(
        "=== DONE: {}/{} ok, {} failed ===",
        ok.len(),
        total,
        fail.len()
    );
    info!("Finalizado: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    Ok(())
}
