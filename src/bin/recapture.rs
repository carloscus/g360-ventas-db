// Recaptura un rango de meses dado (CLI de soporte, reusa el motor completo).
// Uso: cargo run --release --bin recapture -- 2026-06-01 2026-09-30
// Reemplaza los CSVs existentes en raw/ para esos meses y los re-parsea a SQLite.
use anyhow::Result;
use chrono::NaiveDate;
use tracing::{info, error};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use g360_db_ventas::capture_state::{CapturePhase, ProgressState, SharedProgress};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    g360_db_ventas::config::load_dotenv();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Uso: recapture <fecha_inicio YYYY-MM-DD> <fecha_fin YYYY-MM-DD>");
        std::process::exit(1);
    }
    let start = NaiveDate::parse_from_str(&args[1], "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("fecha inicio invalida: {}", e))?;
    let end = NaiveDate::parse_from_str(&args[2], "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("fecha fin invalida: {}", e))?;
    if end < start {
        anyhow::bail!("fecha fin < inicio");
    }

    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    info!("Recapture {} -> {}", start, end);

    // Lock
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;
    let lock_path = raw.join("capture.lock");
    if lock_path.exists() {
        anyhow::bail!("capture.lock activo: {}", lock_path.display());
    }
    std::fs::write(&lock_path, format!("pid={}\nstarted={}\n", std::process::id(), started))?;

    let shared: SharedProgress = std::sync::Arc::new(std::sync::Mutex::new(ProgressState::new()));
    {
        let mut s = shared.lock().unwrap();
        s.set_start(&format!("Recapture {} -> {}", start, end));
    }

    let shared_w = shared.clone();
    let watcher = tokio::spawn(async move {
        let mut last = String::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let st = shared_w.lock().unwrap();
            let line = format!("[{}] {:.0}% {}", st.phase, st.progress * 100.0, st.message);
            if line != last {
                info!("{}", line);
                last = line;
            }
            if st.phase == CapturePhase::Idle || st.phase == CapturePhase::Done || st.phase == CapturePhase::Error {
                break;
            }
        }
    });

    let result = g360_db_ventas::capture::run_batch_history(
        &start.format("%Y-%m-%d").to_string(),
        &end.format("%Y-%m-%d").to_string(),
        false,
        shared,
        None,
    ).await;

    watcher.abort();
    let _ = std::fs::remove_file(&lock_path);

    match result {
        Ok(()) => {
            info!("RECAPTURE OK: {} -> {}", start, end);
            info!("Siguiente paso: reparse automatico ya fue incluido en run_batch_history.");
        }
        Err(e) => error!("RECAPTURE FAIL: {}", e),
    }
    Ok(())
}
