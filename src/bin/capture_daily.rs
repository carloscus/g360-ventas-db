// Captura incremental diaria: solo el mes en curso (del día 1 a hoy).
// Idempotente — reemplaza el mes completo en SQLite si ya existe.
// Uso: cargo run --release --bin capture_daily
//      cargo run --release --bin capture_daily -- 2026-11-05  (hasta fecha específica)
//
// Flujo operativo:
//   1. Correr cada mañana (o vía Task Scheduler de Windows)
//   2. Descarga solo el mes en curso (~segundos, no horas)
//   3. Re-parsea e inserta (DELETE + INSERT del mes)
//   4. Regenera cartucho si hay cambios
//   5. Sync a Supabase (pendiente)
use anyhow::Result;
use chrono::{Datelike, NaiveDate};
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

    // Fecha fin: hoy por defecto, o la que se pase como argumento
    let end = if args.len() > 1 {
        NaiveDate::parse_from_str(&args[1], "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("fecha inválida (YYYY-MM-DD): {}", e))?
    } else {
        chrono::Local::now().date_naive()
    };

    // Fecha inicio: día 1 del mes de `end`
    let start = NaiveDate::from_ymd_opt(end.year(), end.month(), 1)
        .ok_or_else(|| anyhow::anyhow!("no se pudo construir fecha inicio"))?;

    let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    info!("Capture diario: {} -> {} (mes en curso)", start, end);

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
        s.set_start(&format!("Capture diario {} -> {}", start, end));
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
        3,
    ).await;

    watcher.abort();
    let _ = std::fs::remove_file(&lock_path);

    match result {
        Ok(()) => {
            info!("CAPTURE DIARIO OK: {} -> {}", start, end);
            info!("Siguiente paso: generar cartucho + sync Supabase (pendiente).");
        }
        Err(e) => {
            error!("CAPTURE DIARIO FAIL: {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}
