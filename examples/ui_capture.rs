// Harness temporal: simula la llamada exacta de la UI a run_batch_history.
// Uso: cargo run --example ui_capture -- 2026-08-01 2026-08-23
use std::sync::Arc;
use std::sync::Mutex;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();

    let args: Vec<String> = std::env::args().collect();
    let sd = args.get(1).cloned().unwrap_or_default();
    let ed = args.get(2).cloned().unwrap_or_default();
    println!("Simulando UI: capture_range({sd}, {ed})");

    let shared: g360_db_ventas::SharedProgress = Arc::new(Mutex::new(
        g360_db_ventas::capture_state::ProgressState::new(),
    ));

    match g360_db_ventas::capture::run_batch_history(&sd, &ed, false, shared.clone()).await {
        Ok(_) => {
            let st = shared.lock().unwrap();
            println!("OK fase={} msg={}", st.phase, st.message);
        }
        Err(e) => {
            let st = shared.lock().unwrap();
            println!("ERR: {e:#}");
            println!("fase={} msg={}", st.phase, st.message);
            // cadena completa de causas
            let mut src = e.source();
            while let Some(s) = src {
                println!("  causa: {s}");
                src = s.source();
            }
        }
    }
    Ok(())
}
