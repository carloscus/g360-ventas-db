use std::sync::atomic::Ordering;
use std::sync::Arc;

use g360_db_ventas::capture_state::{now_secs, CapturePhase, SharedProgress};

use crate::state::*;
use crate::types::*;

/// Flujo automático inteligente: captura desde la última fecha local, comprueba con
/// Supabase si hubo cambios (fecha más reciente y conteo de filas) y solo entonces sube.
/// No bloquea: corre en background y reporta por el estado de captura (get_capture_status).
#[tauri::command]
pub async fn sync_smart(sync_supabase: Option<bool>) -> Result<CaptureStatus, String> {
    // Si el checkbox "Incluir Supabase" viene marcado (o no se especifica),
    // el flujo termina con sync incremental a Supabase.
    let do_supabase = sync_supabase.unwrap_or(true);
    if let Err(pf) = g360_db_ventas::browser::captor::preflight_checks() {
        return Err(pf);
    }
    let raw = g360_db_ventas::config::raw_dir();
    let lock_path = raw.join("capture.lock");
    if lock_path.exists() {
        let is_running = {
            let st = CAPTURE_STATE.lock().unwrap();
            st.phase != CapturePhase::Idle
                && st.phase != CapturePhase::Done
                && st.phase != CapturePhase::Error
        };
        if is_running {
            return Err("Ya hay una operación en curso. Espere a que termine.".into());
        }
        // Lock huerfano: si otro proceso lo tiene abierto, remove falla -> no
        // lanzar una captura condenada a fallar con "lock ocupado".
        if std::fs::remove_file(&lock_path).is_err() && lock_path.exists() {
            return Err(
                "Otra instancia está capturando en este equipo. Espere a que termine o ciérrela."
                    .into(),
            );
        }
    }

    let shared: SharedProgress = CAPTURE_STATE.clone();
    let last_date = {
        let pool = g360_db_ventas::db::writer::init_pool()
            .await
            .map_err(|e| e.to_string())?;
        // Helper único con buffer -1 día (consistente con sync_from_last y
        // el pipeline del task-scheduler; ver db::freshness).
        g360_db_ventas::db::freshness::last_captured_date(
            &pool,
            g360_db_ventas::db::freshness::CAPTURE_BUFFER_DAYS,
        )
        .await
        .map_err(|e| e.to_string())?
    };

    CAPTURE_ABORT.store(false, Ordering::Relaxed);
    {
        let mut s = shared.lock().unwrap();
        s.set_start(&format!("Sync inteligente desde {}", last_date));
    }

    let shared_bg = shared.clone();
    let start_bg = last_date.clone();
    tauri::async_runtime::spawn(async move {
        // 1) Capturar desde la última fecha local
        if let Err(e) = g360_db_ventas::capture::run_batch_history(
            &start_bg,
            "",
            false,
            shared_bg.clone(),
            Some(CAPTURE_ABORT.clone()),
            3,
        )
        .await
        {
            let mut st = shared_bg.lock().unwrap();
            st.set_phase(CapturePhase::Error, &format!("Error captura: {e}"));
            st.update_progress(0.0, &format!("Error captura: {e}"));
            eprintln!("sync_smart captura: {}", e);
            return;
        }
        invalidate_dashboard_caches();

        // 2) Reconciliar con Supabase (mes por mes, resumible) — opcional
        let cfg = g360_db_ventas::config::load_config();
        if !do_supabase {
            let mut st = shared_bg.lock().unwrap();
            let msg = "Captura local lista (sync Supabase desmarcado)";
            st.set_phase(CapturePhase::Done, msg);
            st.update_progress(1.0, msg);
            return;
        }
        if !cfg.supabase_enabled || !cfg.supabase.is_configured() {
            let mut st = shared_bg.lock().unwrap();
            let msg = if cfg.supabase_enabled {
                "Captura local lista (Supabase no configurado)"
            } else {
                "Captura local lista (modo solo local)"
            };
            st.set_phase(CapturePhase::Done, msg);
            st.update_progress(1.0, msg);
            return;
        }
        let retention = cfg.supabase_retention_days_effective();
        let pool = match g360_db_ventas::db::writer::init_pool().await {
            Ok(p) => p,
            Err(e) => {
                let mut st = shared_bg.lock().unwrap();
                st.set_phase(CapturePhase::Error, &format!("Error BD: {e}"));
                return;
            }
        };
        {
            let mut st = shared_bg.lock().unwrap();
            st.set_phase(CapturePhase::Uploading, "Reconciliando con Supabase...");
            st.update_progress(0.5, "Reconciliando con Supabase...");
        }
        let shared_cb = shared_bg.clone();
        let progress_cb: g360_db_ventas::processor::uploader::ProgressCb =
            Some(Arc::new(move |_b, _t, pct, msg| {
                let mut s = shared_cb.lock().unwrap();
                s.phase = CapturePhase::Uploading;
                s.message = msg.to_string();
                s.progress = pct;
                s.started_at = s.started_at.or_else(|| Some(now_secs()));
                s.finished_at = None;
                drop(s);
            }));
        // Sync INCREMENTAL por día: purga+sube solo los días capturados (no el mes entero).
        let hasta_bg = chrono::Local::now().format("%Y-%m-%d").to_string();
        match g360_db_ventas::processor::uploader::sync_incremental(
            &pool,
            retention,
            &start_bg,
            &hasta_bg,
            &progress_cb,
        )
        .await
        {
            Ok(res) => {
                invalidate_dashboard_caches();
                let mut st = shared_bg.lock().unwrap();
                let msg = format!(
                    "Sync incremental: {} meses OK, {} fallidos, {} filas subidas{}",
                    res.months_synced,
                    res.months_failed,
                    res.rows_uploaded,
                    if res.stopped_early {
                        " (cortado; se reanuda luego)"
                    } else {
                        ""
                    }
                );
                st.set_phase(CapturePhase::Done, &msg);
                st.update_progress(1.0, &msg);
            }
            Err(e) => {
                let mut st = shared_bg.lock().unwrap();
                st.set_phase(CapturePhase::Error, &format!("Error reconcile: {e}"));
                st.update_progress(0.0, &format!("Error reconcile: {e}"));
            }
        }
    });

    Ok(CaptureStatus {
        state: "syncing".into(),
        phase: "checking_lock".into(),
        message: format!("Sync inteligente desde {}", last_date),
        progress: 0.1,
        started_at: Some(now_secs()),
        finished_at: None,
        current_item: String::new(),
        eta_secs: None,
    })
}

#[tauri::command]
pub async fn get_capture_status() -> Result<CaptureStatus, String> {
    let st = CAPTURE_STATE.lock().unwrap();
    Ok(CaptureStatus {
        state: phase_to_state(&st.phase).to_string(),
        phase: st.phase.to_string(),
        message: st.message.clone(),
        progress: st.progress,
        started_at: st.started_at,
        finished_at: st.finished_at,
        current_item: st.current_item.clone(),
        eta_secs: st.eta_secs,
    })
}

// ── Control y Preview ──────────────────────────────────────────────────
#[tauri::command]
pub async fn abort_capture() -> Result<String, String> {
    // Flag global (chequeado dentro de get_and_post_export y entre meses) + local
    g360_db_ventas::capture::CAPTURE_ABORT_GLOBAL.store(true, Ordering::Relaxed);
    CAPTURE_ABORT.store(true, Ordering::Relaxed);
    let raw = g360_db_ventas::config::raw_dir();
    let _ = std::fs::remove_file(raw.join("capture.lock"));
    let mut st = CAPTURE_STATE.lock().unwrap();
    st.set_phase(CapturePhase::Error, "Captura abortada por el usuario");
    st.update_progress(0.0, "Abortada");
    Ok("Captura detenida".into())
}
