use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use g360_db_ventas::capture_state::{CapturePhase, SharedProgress, now_secs};

use crate::state::*;
use crate::types::*;

#[tauri::command]
pub async fn capture_range(startDate: String, endDate: String) -> Result<CaptureStatus, String> {
    if let Err(pf) = g360_db_ventas::browser::captor::preflight_checks() {
        return Err(pf);
    }
    // Verificar lock — con deteccion de stale lock (proceso muerto)
    let raw = g360_db_ventas::config::raw_dir();
    let lock_path = raw.join("capture.lock");
    if lock_path.exists() {
        let is_running = {
            let st = CAPTURE_STATE.lock().unwrap();
            st.phase != CapturePhase::Idle && st.phase != CapturePhase::Done && st.phase != CapturePhase::Error
        };
        if !is_running {
            // Lock huerfano: borrar inmediato (el Drop ya intentó, pero Windows puede dejarlo)
            let _ = std::fs::remove_file(&lock_path);
            if lock_path.exists() {
                // si aún existe por handle zombie, solo avisar 5s no 120s
                if let Ok(meta) = std::fs::metadata(&lock_path) {
                    if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed().map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))) {
                        if elapsed.as_secs() < 5 {
                            return Err(format!("Lock liberándose (hace {}s). Reintente en 3s.", elapsed.as_secs()));
                        }
                        let _ = std::fs::remove_file(&lock_path);
                    }
                }
            }
        } else if let Ok(meta) = std::fs::metadata(&lock_path) {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed().map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))) {
                let mins = elapsed.as_secs() / 60;
                return Err(format!(
                    "Ya hay una captura en curso (hace {}m {}s). Use ⏹ detener o espere.",
                    mins, elapsed.as_secs() % 60
                ));
            }
            return Err("Ya hay una captura en curso. Use ⏹ detener.".into());
        }
    }

    let shared: SharedProgress = CAPTURE_STATE.clone();
    let sd = startDate.clone();
    let ed = endDate.clone();

    let mut s = shared.lock().unwrap();
    let quien = g360_db_ventas::config::load_config().generado_por;
    let quien_txt = if quien.is_empty() { String::new() } else { format!(" (por {quien})") };
    s.set_start(&format!("Captura rango {} -> {}{}", startDate, endDate, quien_txt));
    drop(s);

    // Resetear flag de abort
    CAPTURE_ABORT.store(false, Ordering::Relaxed);

    let shared_err = shared.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = g360_db_ventas::capture::run_batch_history(&sd, &ed, false, shared, Some(CAPTURE_ABORT.clone()), 3).await {
            let mut st = shared_err.lock().unwrap();
            st.set_phase(CapturePhase::Error, &format!("Error: {e}"));
            st.update_progress(0.0, &format!("Error: {e}"));
            eprintln!("capture_range error: {}", e);
        }
    });
    Ok(CaptureStatus {
        state: "syncing".into(),
        phase: "checking_lock".into(),
        message: format!("Rango {} -> {} iniciado", startDate, endDate),
        progress: 0.1,
        started_at: Some(now_secs()),
        finished_at: None,
        current_item: String::new(),
        eta_secs: None,
    })
}

#[tauri::command]
pub async fn sync_from_last() -> Result<CaptureStatus, String> {
    if let Err(pf) = g360_db_ventas::browser::captor::preflight_checks() {
        return Err(pf);
    }
    let raw = g360_db_ventas::config::raw_dir();
    let lock_path = raw.join("capture.lock");
    if lock_path.exists() {
        let is_running = {
            let st = CAPTURE_STATE.lock().unwrap();
            st.phase != CapturePhase::Idle && st.phase != CapturePhase::Done && st.phase != CapturePhase::Error
        };
        if !is_running {
            // Lock huerfano (esta app no captura): intentar borrarlo. Si otro
            // proceso lo tiene abierto (share_mode 0), remove falla -> no lanzar
            // una captura condenada a fallar con "lock ocupado".
            if std::fs::remove_file(&lock_path).is_err() && lock_path.exists() {
                if let Ok(meta) = std::fs::metadata(&lock_path) {
                    if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed().map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))) {
                        if elapsed.as_secs() < 5 {
                            return Err(format!("Lock liberándose (hace {}s). Reintente en 3s.", elapsed.as_secs()));
                        }
                    }
                }
                return Err("Otra instancia está capturando en este equipo. Espere a que termine o ciérrela.".into());
            }
        } else if let Ok(meta) = std::fs::metadata(&lock_path) {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed().map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))) {
                let mins = elapsed.as_secs() / 60;
                return Err(format!(
                    "Ya hay una captura en curso (hace {}m {}s). Use ⏹ detener o espere.",
                    mins, elapsed.as_secs() % 60
                ));
            }
            return Err("Ya hay una captura en curso. Use ⏹ detener.".into());
        }
    }
    let shared: SharedProgress = CAPTURE_STATE.clone();

    // Consultar la ultima fecha capturada para sincronizar desde ahi
    let last_date = {
        let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
        let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
            .fetch_one(&pool)
            .await
            .unwrap_or(None);
        last.unwrap_or_else(|| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            // Formato yyyy-mm-dd en hora Peru (UTC-5)
            let secs_pe = now.saturating_sub(5 * 3600);
            let days = (secs_pe / 86400) as i64;
            let z = days + 719468;
            let era = if z >= 0 { z } else { z - 146096 } / 146097;
            let doe = (z - era * 146097) as i64;
            let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
            let mut y = yoe + era * 400;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            if m <= 2 { y += 1; }
            format!("{:04}-{:02}-{:02}", y, m, d)
        })
    };

    let mut s = shared.lock().unwrap();
    s.set_start(&format!("Sync desde {}", last_date));
    drop(s);

    // Resetear flag de abort
    CAPTURE_ABORT.store(false, Ordering::Relaxed);

    let shared_err = shared.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = g360_db_ventas::capture::run_batch_history(&last_date, "", false, shared, Some(CAPTURE_ABORT.clone()), 3).await {
            let mut st = shared_err.lock().unwrap();
            st.set_phase(CapturePhase::Error, &format!("Error: {e}"));
            st.update_progress(0.0, &format!("Error: {e}"));
            eprintln!("sync_from_last error: {}", e);
        }
    });
    Ok(CaptureStatus {
        state: "syncing".into(),
        phase: "checking_lock".into(),
        message: "Sync desde ultima captura".into(),
        progress: 0.1,
        started_at: Some(now_secs()),
        finished_at: None,
        current_item: String::new(),
        eta_secs: None,
    })
}

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
            return Err("Otra instancia está capturando en este equipo. Espere a que termine o ciérrela.".into());
        }
    }

    let shared: SharedProgress = CAPTURE_STATE.clone();
    let last_date = {
        let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
        let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
            .fetch_one(&pool)
            .await
            .unwrap_or(None);
        match last {
            Some(d) => {
                // Buffer -1 día: cubre documentos posteados con retraso en el ERP
                if let Ok(dt) = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
                    (dt - chrono::Duration::days(1)).format("%Y-%m-%d").to_string()
                } else {
                    d
                }
            }
            None => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let secs_pe = now.saturating_sub(5 * 3600);
                let days = (secs_pe / 86400) as i64;
                let z = days + 719468;
                let era = if z >= 0 { z } else { z - 146096 } / 146097;
                let doe = (z - era * 146097) as i64;
                let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                let mut y = yoe + era * 400;
                let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                let mp = (5 * doy + 2) / 153;
                let d = doy - (153 * mp + 2) / 5 + 1;
                let m = if mp < 10 { mp + 3 } else { mp - 9 };
                if m <= 2 { y += 1; }
                format!("{:04}-{:02}-{:02}", y, m, d)
            }
        }
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
            &start_bg, "", false, shared_bg.clone(), Some(CAPTURE_ABORT.clone()), 3,
        ).await {
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
                    if res.stopped_early { " (cortado; se reanuda luego)" } else { "" }
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

/// Flujo automatizado completo: descarga/captura del rango desde la intranet,
/// normaliza a SQLite y reconcilia Supabase (mes por mes, sin datos repetidos).
/// Corre en background y reporta por el estado de captura (get_capture_status).
#[tauri::command]
pub async fn run_full_flow(desde: String, hasta: String) -> Result<CaptureStatus, String> {
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
        if std::fs::remove_file(&lock_path).is_err() && lock_path.exists() {
            return Err("Otra instancia está capturando en este equipo. Espere o ciérrela.".into());
        }
    }

    let shared: SharedProgress = CAPTURE_STATE.clone();
    CAPTURE_ABORT.store(false, Ordering::Relaxed);
    {
        let mut s = shared.lock().unwrap();
        s.set_start(&format!("Flujo completo {} -> {}", desde, hasta));
    }

    let shared_bg = shared.clone();
    let (d, h) = (desde.clone(), hasta.clone());
    tauri::async_runtime::spawn(async move {
        // 1) Captura + normalizacion a SQLite
        if let Err(e) = g360_db_ventas::capture::run_batch_history(
            &d, &h, false, shared_bg.clone(), Some(CAPTURE_ABORT.clone()), 3,
        ).await {
            let mut st = shared_bg.lock().unwrap();
            st.set_phase(CapturePhase::Error, &format!("Error captura: {e}"));
            st.update_progress(0.0, &format!("Error captura: {e}"));
            return;
        }
        invalidate_dashboard_caches();

        // 2) Reconciliacion con Supabase (opcional)
        let cfg = g360_db_ventas::config::load_config();
        if !cfg.supabase_enabled || !cfg.supabase.is_configured() {
            let mut st = shared_bg.lock().unwrap();
            st.set_phase(CapturePhase::Done, "Flujo local completado (Supabase deshabilitado)");
            st.update_progress(1.0, "Flujo local completado (Supabase deshabilitado)");
            return;
        }
        let pool = match g360_db_ventas::db::writer::init_pool().await {
            Ok(p) => p,
            Err(e) => {
                let mut st = shared_bg.lock().unwrap();
                st.set_phase(CapturePhase::Error, &format!("Error BD: {e}"));
                return;
            }
        };
        let retention = cfg.supabase_retention_days_effective();
        {
            let mut st = shared_bg.lock().unwrap();
            st.set_phase(CapturePhase::Uploading, "Reconciliando con Supabase...");
            st.update_progress(0.6, "Reconciliando con Supabase...");
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
        match g360_db_ventas::processor::uploader::sync_incremental(&pool, retention, &d, &h, &progress_cb).await {
            Ok(res) => {
                invalidate_dashboard_caches();
                let msg = format!(
                    "Flujo completo: {} meses OK, {} filas subidas, {} purgadas{}",
                    res.months_synced, res.rows_uploaded, res.rows_purged,
                    if res.stopped_early { " (cortado; se reanuda luego)" } else { "" }
                );
                let mut st = shared_bg.lock().unwrap();
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
        message: format!("Flujo completo {} -> {}", desde, hasta),
        progress: 0.1,
        started_at: Some(now_secs()),
        finished_at: None,
        current_item: String::new(),
        eta_secs: None,
    })
}

// ── Gestion de BD local ────────────────────────────────────────────────
#[tauri::command]
pub async fn clear_cache() -> Result<CaptureStatus, String> {
    let raw = g360_db_ventas::config::raw_dir();
    let _ = std::fs::remove_file(raw.join("capture.lock"));
    let mut removed = 0usize;
    if let Ok(entries) = std::fs::read_dir(&raw) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("xls") {
                let csv = p.with_extension("csv");
                if csv.exists() && std::fs::metadata(&csv).map(|m| m.len() > 1000).unwrap_or(false) {
                    let _ = std::fs::remove_file(&p);
                    removed += 1;
                }
            }
            if p.file_name().and_then(|s| s.to_str()).map(|s| s.contains("-p") && s.ends_with(".csv")).unwrap_or(false) {
                let _ = std::fs::remove_file(&p);
                removed += 1;
            }
        }
    }
    let msg = format!("Cache limpiada: {} temporales", removed);
    set_state("idle", &msg, 1.0);
    Ok(CaptureStatus {
        state: "idle".into(),
        phase: "idle".into(),
        message: msg,
        progress: 1.0,
        started_at: None,
        finished_at: Some(now_secs()),
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

