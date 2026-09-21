use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();
    let cfg = g360_db_ventas::config::load_config();
    info!("g360-db-ventas - Reconciliacion Supabase (mes por mes)");
    if !cfg.supabase_enabled {
        info!("Supabase deshabilitado (modo solo local); nada que hacer.");
        return Ok(());
    }
    let pool = g360_db_ventas::db::writer::init_pool().await?;
    let retention = cfg.supabase_retention_days_effective();
    match g360_db_ventas::processor::uploader::reconcile_supabase(&pool, retention, &None).await {
        Ok(res) => {
            info!(
                "Reconciliacion OK: {} meses sincronizados, {} fallidos, {} filas subidas, {} purgadas{} (retencion {} dias)",
                res.months_synced, res.months_failed, res.rows_uploaded, res.rows_purged,
                if res.stopped_early { " [cortado; se reanuda luego]" } else { "" },
                retention
            );
        }
        Err(e) => error!("Reconciliacion fail: {}", e),
    }
    Ok(())
}
