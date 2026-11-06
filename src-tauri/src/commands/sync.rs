use std::sync::Arc;

use g360_db_ventas::capture_state::{CapturePhase, SharedProgress, now_secs};

use crate::state::*;

#[tauri::command]
pub async fn reset_sync_marker() -> Result<String, String> {
    let mut cfg = g360_db_ventas::config::load_config();
    cfg.last_supabase_sync = None;
    g360_db_ventas::config::save_config(&cfg).map_err(|e| e.to_string())?;
    // Fuerza reconciliacion MES POR MES (no purga de ventana completa).
    // Se usa '' (no NULL) para distinguir un reset FORZADO del estado inicial de una
    // BD recreada: el sembrado solo llena NULLs, asi que '' fuerza la re-subida.
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE sync_months SET remote_checksum = '', estado = 'pending'")
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok("Marcador reseteado. La próxima sincronización reconciliará mes por mes.".to_string())
}

/// Reconciliación por mes con Supabase (resumible, con circuit breaker).
/// Reemplaza el Full Sync de ventana completa. No-op si Supabase está deshabilitado.
#[tauri::command]
pub async fn reconcile_supabase_cmd(
) -> Result<g360_db_ventas::processor::uploader::ReconcileResult, String> {
    let cfg = g360_db_ventas::config::load_config();
    if !cfg.supabase_enabled {
        return Err("Supabase deshabilitado (modo solo local). Actívalo en Configuración.".into());
    }
    if !cfg.supabase.is_configured() {
        return Err("Supabase no configurado".into());
    }
    {
        let st = CAPTURE_STATE.lock().unwrap();
        if st.phase != CapturePhase::Idle
            && st.phase != CapturePhase::Done
            && st.phase != CapturePhase::Error
        {
            return Err("Hay una captura en curso. Espere a que termine antes de reconciliar.".into());
        }
    }
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let retention = cfg.supabase_retention_days_effective();
    let shared: SharedProgress = CAPTURE_STATE.clone();
    let progress_cb: g360_db_ventas::processor::uploader::ProgressCb =
        Some(Arc::new(move |_b, _t, pct, msg| {
            let mut s = shared.lock().unwrap();
            s.phase = CapturePhase::Uploading;
            s.message = msg.to_string();
            s.progress = pct;
            s.started_at = s.started_at.or_else(|| Some(now_secs()));
            s.finished_at = None;
            drop(s);
        }));
    let res = g360_db_ventas::processor::uploader::reconcile_supabase(&pool, retention, &progress_cb)
        .await
        .map_err(|e| e.to_string())?;
    invalidate_dashboard_caches();
    // Registrar la fecha de última sincronización (aunque no hubiera meses pendientes:
    // Supabase quedó verificado contra el local en este momento).
    {
        let mut cfg2 = g360_db_ventas::config::load_config();
        cfg2.last_supabase_sync = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
        let _ = g360_db_ventas::config::save_config(&cfg2);
    }
    let msg = format!(
        "Reconciliación: {} meses sincronizados, {} fallidos, {} filas subidas{}",
        res.months_synced,
        res.months_failed,
        res.rows_uploaded,
        if res.stopped_early { " (cortado por errores; se reanuda en la próxima)" } else { "" }
    );
    set_state("idle", &msg, 1.0);
    Ok(res)
}

#[tauri::command]
pub async fn upload_all() -> Result<String, String> {
    // Reconciliación por mes (resumible) en vez de full-window purge.
    let res = reconcile_supabase_cmd().await?;
    Ok(format!(
        "Reconciliación: {} meses sincronizados, {} fallidos, {} filas subidas, {} purgadas{}",
        res.months_synced,
        res.months_failed,
        res.rows_uploaded,
        res.rows_purged,
        if res.stopped_early { " (cortado; se reanuda luego)" } else { "" }
    ))
}

/// Dry-run: muestra qué se subiría sin hacer upload real
#[tauri::command]
pub async fn upload_dry_run() -> Result<serde_json::Value, String> {
    let cfg = g360_db_ventas::config::load_config();
    if !cfg.supabase.is_configured() {
        return Err("Supabase no configurado".to_string());
    }
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let retention = cfg.supabase_retention_days_effective();
    let last_sync = cfg.last_supabase_sync.clone();

    let dry_run = g360_db_ventas::processor::uploader::dry_run_upload(&pool, retention, last_sync.as_deref())
        .await
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "rows_to_upload": dry_run.rows_to_upload,
        "total_batches": dry_run.total_batches,
        "total_cantidad": dry_run.total_cantidad,
        "total_soles": dry_run.total_soles,
        "total_dolares": dry_run.total_dolares,
        "unique_invoices": dry_run.unique_invoices,
        "date_range": format!("{:?} a {:?}", dry_run.date_range_start, dry_run.date_range_end),
        "estimated_size_mb": (dry_run.rows_to_upload as f64 * 0.7).round() / 1000.0,
        "within_limit": (dry_run.rows_to_upload as f64 * 0.7) < 500_000_000.0
    }))
}

/// Sincroniza con Supabase reconciliando MES POR MES (resumible, circuit breaker).
/// Reemplaza el upload/full-sync anterior. No-op si Supabase está deshabilitado.
#[tauri::command]
pub async fn upload_all_with_verify() -> Result<serde_json::Value, String> {
    let res = reconcile_supabase_cmd().await?;
    Ok(serde_json::json!({
        "status": if res.stopped_early { "partial" } else { "ok" },
        "message": format!(
            "Reconciliación: {} meses sincronizados, {} fallidos, {} filas subidas, {} purgadas{}",
            res.months_synced, res.months_failed, res.rows_uploaded, res.rows_purged,
            if res.stopped_early { " (reanuda en la próxima)" } else { "" }
        ),
        "months_total": res.months_total,
        "months_synced": res.months_synced,
        "months_failed": res.months_failed,
        "rows_uploaded": res.rows_uploaded,
        "rows_purged": res.rows_purged,
        "stopped_early": res.stopped_early
    }))
}

// ─── AUTOMATIZACIÓN DIARIA ───────────────────────────────────────────────

/// Pipeline diario automático: captura + parseo + validación + sync
#[tauri::command]
pub async fn auto_daily_pipeline() -> Result<serde_json::Value, String> {
    let cfg = g360_db_ventas::config::load_config();

    if !cfg.supabase.is_configured() {
        return Err("Supabase no configurado".to_string());
    }
    if cfg.intranet.user.is_empty() || cfg.intranet.pass.is_empty() {
        return Err("Credenciales de intranet no configuradas".to_string());
    }

    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;

    // Paso 1: Capturar día anterior
    set_state("syncing", "Capturando datos del día anterior...", 0.1);

    let yesterday = chrono::Utc::now().date_naive() - chrono::Duration::days(1);
    let yesterday_str = yesterday.format("%Y-%m-%d").to_string();

    let capture_result = g360_db_ventas::capture::run_batch_history(&yesterday_str, "", false, CAPTURE_STATE.clone(), None, 3).await;

    match capture_result {
        Ok(_) => {
            eprintln!("Captura diaria completada: {}", yesterday_str);

            // Paso 2: Verificar integridad
            set_state("syncing", "Verificando integridad...", 0.5);
            let integrity = g360_db_ventas::db::writer::verify_integrity(&pool).await.unwrap_or_default();

            // Paso 3: Calcular checksums
            set_state("syncing", "Calculando checksums...", 0.7);
            let _ = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool).await;

            // Paso 4: Sync incremental por día (solo el día capturado)
            set_state("uploading", "Sincronizando con Supabase...", 0.8);
            let shared: SharedProgress = CAPTURE_STATE.clone();
            let progress_cb: g360_db_ventas::processor::uploader::ProgressCb = Some(Arc::new(move |_b, _t, pct, msg| {
                let mut s = shared.lock().unwrap();
                s.phase = CapturePhase::Uploading;
                s.message = msg.to_string();
                s.progress = pct;
                drop(s);
            }));
            match g360_db_ventas::processor::uploader::sync_incremental(&pool, cfg.supabase_retention_days_effective(), &yesterday_str, &yesterday_str, &progress_cb).await {
                Ok(res) => {
                    set_state("idle", &format!("Pipeline diario: {} meses OK, {} filas subidas", res.months_synced, res.rows_uploaded), 1.0);
                    Ok(serde_json::json!({
                        "status": if res.stopped_early { "partial" } else { "ok" },
                        "message": format!("Pipeline completado: {} capturado, {} filas subidas, {} meses OK{}", yesterday_str, res.rows_uploaded, res.months_synced, if res.stopped_early { " (cortado)" } else { "" }),
                        "captured_date": yesterday_str,
                        "rows_uploaded": res.rows_uploaded,
                        "rows_purged": res.rows_purged,
                        "months_synced": res.months_synced,
                        "months_failed": res.months_failed,
                        "stopped_early": res.stopped_early,
                        "integrity_issues": integrity.len()
                    }))
                }
                Err(e) => {
                    set_state("idle", &format!("Error en reconciliación: {}", e), 0.0);
                    Err(format!("Error en reconciliación: {}", e))
                }
            }
        }
        Err(e) => {
            set_state("idle", &format!("Error en captura: {}", e), 0.0);
            Err(format!("Error en captura: {}", e))
        }
    }
}

/// Obtiene el próximo horario de ejecución programada
#[tauri::command]
pub async fn get_next_capture_time() -> Result<serde_json::Value, String> {
    let cfg = g360_db_ventas::config::load_config();
    let now = chrono::Local::now();

    let mut next_times = Vec::new();
    for time_str in &cfg.capture_times {
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() == 2 {
            if let (Ok(hour), Ok(min)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                let now_naive = now.naive_local();
                let mut next = now_naive.date().and_hms_opt(hour, min, 0).unwrap_or(now_naive);
                if next <= now_naive {
                    next += chrono::Duration::days(1);
                }
                let hours_until = (next - now_naive).num_seconds() / 3600;
                next_times.push(serde_json::json!({
                    "time": time_str,
                    "next_execution": next.format("%Y-%m-%dT%H:%M:%S").to_string(),
                    "hours_until": hours_until
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "auto_daily_capture": cfg.auto_daily_capture,
        "capture_times": cfg.capture_times,
        "next_executions": next_times,
        "app_retention_years": cfg.app_retention_years,
        "supabase_retention_years": cfg.supabase_retention_years
    }))
}

// ─── MANTENIMIENTO BD ─────────────────────────────────────────────────

