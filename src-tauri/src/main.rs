#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};
use std::env;

use g360_db_ventas::capture_state::{CapturePhase, ProgressState, SharedProgress, now_secs};

static CAPTURE_STATE: LazyLock<SharedProgress> =
    LazyLock::new(|| std::sync::Arc::new(std::sync::Mutex::new(ProgressState::new())));

/// Flag para abortar captura en curso. Se consulta en el loop de capture.
static CAPTURE_ABORT: LazyLock<Arc<AtomicBool>> =
    LazyLock::new(|| Arc::new(AtomicBool::new(false)));

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardStats {
    pub total_records: i64,
    pub total_sales: f64,
    pub total_clients: i64,
    pub total_skus: i64,
    pub months: Vec<MonthStats>,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct MonthStats {
    pub mes_ref: String,
    pub rows: i64,
    pub sales: f64,
    pub clients: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CaptureStatus {
    pub state: String,
    pub phase: String,
    pub message: String,
    pub progress: f32,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub current_item: String,
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthStats {
    pub total_records: i64,
    pub min_date: String,
    pub max_date: String,
    pub last_capture: String,
    pub db_size_mb: f64,
    pub status: String,
    pub db_path: String,
    pub db_exists: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    pub supabase_url: String,
    pub supabase_key: String,
    pub supabase_service_role_configured: bool,
    pub supabase_table: String,
    pub supabase_configured: bool,
    pub intranet_user: String,
    pub intranet_pass: String,
    pub generado_por: String,
    pub allowed_lines: String,
    pub auto_sync: bool,
    pub app_retention_years: u32,
    pub supabase_retention_years: u32,
    pub supabase_retention_days: u32,
    pub last_supabase_sync: Option<String>,
    pub auto_daily_capture: bool,
    pub capture_times: Vec<String>,
    pub db_path: String,
}

fn set_state(state: &str, msg: &str, progress: f32) {
    let mut s = CAPTURE_STATE.lock().unwrap();
    s.phase = match state {
        "idle" => CapturePhase::Idle,
        "uploading" => CapturePhase::Uploading,
        _ => CapturePhase::Downloading,
    };
    s.message = msg.to_string();
    s.progress = progress;
    if state == "syncing" || state == "uploading" {
        s.started_at = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
        s.finished_at = None;
    } else if state == "idle" {
        s.finished_at = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
    }
}

// ── Helpers de estado ──────────────────────────────────────────────────
/// Map internal CapturePhase to frontend-compatible state string
fn phase_to_state(phase: &CapturePhase) -> &'static str {
    match phase {
        CapturePhase::Idle => "idle",
        CapturePhase::CheckingLock => "syncing",
        CapturePhase::Downloading => "syncing",
        CapturePhase::Parsing => "syncing",
        CapturePhase::Normalizing => "syncing",
        CapturePhase::Uploading => "uploading",
        CapturePhase::Done => "idle",
        CapturePhase::Error => "error",
    }
}

// ── Dashboard y Health ─────────────────────────────────────────────────
#[tauri::command]
async fn get_dashboard() -> Result<DashboardStats, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    // Stats cache: evita full-scan de 1.1M filas en queries lentas.
    // La columna value es REAL -> leer como f64 y castear (REAL->i64 falla en sqlx).
    let total_records: i64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_records'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as i64;
    let total_sales: f64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_sales'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as f64;
    let total_clients: i64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_clients'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as i64;
    let total_skus: i64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_skus'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as i64;
    let months: Vec<MonthStats> = sqlx::query_as("SELECT mes_ref, COUNT(*) as rows, COALESCE(SUM(soles), 0.0) as sales, COUNT(DISTINCT id_cliente) as clients FROM ventas GROUP BY mes_ref ORDER BY mes_ref").fetch_all(&pool).await.map_err(|e| e.to_string())?;
    Ok(DashboardStats { total_records, total_sales, total_clients, total_skus, months })
}

/// Unix secs -> "dd/mm/yyyy HH:MM:SS" en hora Peru (GMT-5)
fn fmt_hora_local(secs: u64) -> String {
    // Peru: UTC-5 sin horario de verano
    let secs_pe = secs.saturating_sub(5 * 3600);
    let days = (secs_pe / 86400) as i64;
    let rem = secs_pe % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
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
    format!("{:02}/{:02}/{:04} {:02}:{:02}:{:02}", d, m, y, h, mi, s)
}

#[tauri::command]
async fn get_health() -> Result<HealthStats, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let total_records: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let min_date: Option<String> = sqlx::query_scalar("SELECT MIN(fecha_orig) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let max_date: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let db_path = g360_db_ventas::config::db_path();
    let db_exists = db_path.exists();
    let db_size_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1024.0 / 1024.0).unwrap_or(0.0);
    let last_capture = std::fs::metadata(&db_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| fmt_hora_local(d.as_secs()))
        .unwrap_or_else(|| "n/a".into());
    let st = CAPTURE_STATE.lock().unwrap();
    let running = st.phase != CapturePhase::Idle && st.phase != CapturePhase::Done && st.phase != CapturePhase::Error;
    let lock_path = g360_db_ventas::config::raw_dir().join("capture.lock");
    let lock_exists = lock_path.exists();
    // Auto-limpiar lock huerfano en health check (no bloquear UI con STALE)
    let stale_lock = if lock_exists && !running {
        // Si lleva >2s sin running, es huerfano del Drop fallido en Windows — borrar
        if let Ok(meta) = std::fs::metadata(&lock_path) {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed().map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))) {
                if elapsed.as_secs() > 2 {
                    let _ = std::fs::remove_file(&lock_path);
                    false
                } else { true }
            } else { let _ = std::fs::remove_file(&lock_path); false }
        } else { false }
    } else { false };
    let status = if lock_exists && stale_lock {
        "STALE_LOCK".to_string()
    } else if running {
        "SYNCING".to_string()
    } else if total_records == 0 {
        "EMPTY".to_string()
    } else {
        "OK".to_string()
    };
    drop(st);
    Ok(HealthStats { total_records, min_date: min_date.unwrap_or_default(), max_date: max_date.unwrap_or_default(), last_capture, db_size_mb, status, db_path: db_path.to_string_lossy().to_string(), db_exists })
}

// ── Settings y Supabase ────────────────────────────────────────────────
#[tauri::command]
async fn get_settings() -> Result<AppSettings, String> {
    let cfg = g360_db_ventas::config::load_config();
    let db_path = g360_db_ventas::config::db_path().to_string_lossy().to_string();
    let sb_retention_days_eff = cfg.supabase_retention_days_effective();
    Ok(AppSettings {
        supabase_url: cfg.supabase.url.clone(),
        supabase_key: cfg.supabase.key.clone(),
        supabase_service_role_configured: !cfg.supabase.service_role_key.is_empty(),
        supabase_table: g360_db_ventas::config::SUPABASE_TABLE.to_string(),
        supabase_configured: cfg.supabase.is_configured(),
        intranet_user: if cfg.intranet.user.is_empty() {
            std::env::var("G360_INTRANET_USER").unwrap_or_default()
        } else { cfg.intranet.user },
        intranet_pass: String::new(),
        generado_por: cfg.generado_por,
        allowed_lines: cfg.allowed_lines.join(", "),
        auto_sync: cfg.auto_sync,
        app_retention_years: cfg.app_retention_years,
        supabase_retention_years: sb_retention_days_eff,
        supabase_retention_days: cfg.supabase_retention_days,
        last_supabase_sync: cfg.last_supabase_sync.clone(),
        auto_daily_capture: cfg.auto_daily_capture,
        capture_times: cfg.capture_times.clone(),
        db_path,
    })
}

#[tauri::command]
async fn save_settings(
    supabase_url: String,
    supabase_key: String,
    intranet_user: String,
    intranet_pass: String,
    generado_por: String,
    allowed_lines: String,
    auto_sync: bool,
    app_retention_years: u32,
    supabase_retention_years: u32,
    supabase_retention_days: u32,
    auto_daily_capture: bool,
    capture_times: Vec<String>,
) -> Result<AppSettings, String> {
    let mut cfg = g360_db_ventas::config::load_config();
    cfg.supabase.url = supabase_url;
    if !supabase_key.trim().is_empty() {
        cfg.supabase.key = supabase_key;
    }
    cfg.intranet.user = intranet_user;
    if !intranet_pass.trim().is_empty() {
        cfg.intranet.pass = intranet_pass;
    }
    cfg.generado_por = generado_por;
    cfg.allowed_lines = allowed_lines
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
     cfg.auto_sync = auto_sync;
     cfg.app_retention_years = app_retention_years;
     cfg.supabase_retention_years = supabase_retention_years;
     cfg.supabase_retention_days = supabase_retention_days;
     cfg.auto_daily_capture = auto_daily_capture;
     if !capture_times.is_empty() {
         cfg.capture_times = capture_times;
     }
     g360_db_ventas::config::save_config(&cfg).map_err(|e| e.to_string())?;
    let db_path = g360_db_ventas::config::db_path().to_string_lossy().to_string();
    let sb_retention_days_eff = cfg.supabase_retention_days_effective();
    Ok(AppSettings {
        supabase_url: cfg.supabase.url.clone(),
        supabase_key: String::new(),
        supabase_service_role_configured: !cfg.supabase.service_role_key.is_empty(),
        supabase_table: g360_db_ventas::config::SUPABASE_TABLE.to_string(),
        supabase_configured: cfg.supabase.is_configured(),
        intranet_user: cfg.intranet.user,
        intranet_pass: String::new(),
        generado_por: cfg.generado_por,
        allowed_lines: cfg.allowed_lines.join(", "),
        auto_sync: cfg.auto_sync,
        app_retention_years: cfg.app_retention_years,
        supabase_retention_years: sb_retention_days_eff,
        supabase_retention_days: cfg.supabase_retention_days,
        last_supabase_sync: cfg.last_supabase_sync.clone(),
        auto_daily_capture: cfg.auto_daily_capture,
        capture_times: cfg.capture_times.clone(),
        db_path,
    })
}

#[tauri::command]
async fn test_supabase(url: String, key: String) -> Result<String, String> {
    let res = g360_db_ventas::processor::uploader::test_supabase_connection(&url, &key).await.map_err(|e| e.to_string())?;
    Ok(res)
}

#[tauri::command]
async fn reset_sync_marker() -> Result<String, String> {
    let mut cfg = g360_db_ventas::config::load_config();
    cfg.last_supabase_sync = None;
    g360_db_ventas::config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok("Marcador de sync reseteado. La próxima subida será completa (Full Sync).".to_string())
}

/// Reset admin: backup de config actual, genera config limpio con credenciales vacías.
/// Para intervención de tercero (admin/support) que necesita reconfigurar acceso.
#[tauri::command]
async fn admin_reset() -> Result<String, String> {
    let cfg = g360_db_ventas::config::load_config();
    // Backup antes de destruir
    let backup_dir = g360_db_ventas::config::data_dir().join("backup");
    let _ = std::fs::create_dir_all(&backup_dir);
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let backup_path = backup_dir.join(format!("config_{}.json", ts));
    let _ = std::fs::write(&backup_path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
    // Config limpio: credenciales vacías, conserva allowed_lines y retención
    let mut new_cfg = g360_db_ventas::config::AppConfig::default_app();
    new_cfg.allowed_lines = cfg.allowed_lines.clone();
    new_cfg.app_retention_years = cfg.app_retention_years;
    new_cfg.supabase_retention_years = cfg.supabase_retention_years;
    new_cfg.auto_sync = cfg.auto_sync;
    new_cfg.last_supabase_sync = None; // forzar full sync
    // generado_por queda vacío — el admin lo rellena
    g360_db_ventas::config::save_config(&new_cfg).map_err(|e| e.to_string())?;
    Ok(format!("Config reseteada. Backup en {}. Reingrese credenciales en Configuración.", backup_path.display()))
}

// ── Captura y Sync ─────────────────────────────────────────────────────

/// Verifica que las credenciales de intranet son válidas (login real al ERP).
/// Es el gate de seguridad: si no pasa, el usuario no es legítimo y no se sincroniza.
async fn verify_intranet_credentials() -> Result<String, String> {
    use g360_db_ventas::browser::http::CipsaHttp;
    let http = CipsaHttp::new().map_err(|e| format!("Error creando cliente HTTP: {}", e))?;
    http.login().await.map_err(|e| format!("Credenciales de intranet invalidas o red no accesible: {}", e))?;
    Ok("OK".to_string())
}

#[tauri::command]
async fn upload_all() -> Result<String, String> {
    let cfg = g360_db_ventas::config::load_config();
    if !cfg.supabase.is_configured() {
        return Err("Supabase no configurado. Abra Configuracion e ingrese credenciales.".to_string());
    }
    set_state("uploading", "Conectando a Supabase...", 0.03);
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;

    let base_where = match &cfg.last_supabase_sync {
        Some(s) if !s.is_empty() => format!("WHERE capturado_en > '{}'", s),
        _ => String::new(),
    };
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM ventas {}", base_where))
        .fetch_one(&pool).await.map_err(|e| e.to_string())?;
    if count == 0 {
        set_state("idle", "Nada nuevo para subir (sync incremental)", 1.0);
        return Ok("Sync incremental: nada nuevo para subir".to_string());
    }

    let last_sync = cfg.last_supabase_sync.clone();
    let retention = cfg.supabase_retention_days_effective(); // Usar retención configurada para Supabase
    let shared: SharedProgress = CAPTURE_STATE.clone();

    // Closure que actualiza el estado de progreso desde el callback del uploader
    let progress_cb: g360_db_ventas::processor::uploader::ProgressCb = Some(Arc::new(move |batch, total, pct, msg| {
        let mut s = shared.lock().unwrap();
        s.phase = CapturePhase::Uploading;
        s.message = msg.to_string();
        s.progress = pct;
        s.started_at = s.started_at.or_else(|| Some(now_secs()));
        s.finished_at = None;
        drop(s);
    }));

    set_state("uploading", &format!("Subiendo {} registros a Supabase...", count), 0.05);

    match g360_db_ventas::processor::uploader::upload_all(&pool, retention, last_sync.as_deref(), &progress_cb).await {
        Ok((up, cleaned)) => {
            // Solo avanzar el marcador si realmente se subieron filas.
            // Evita el caso donde se marca last_supabase_sync sin subir nada
            // (bloquea futuros syncs incrementales con "nada nuevo").
            if up > 0 {
                let mut cfg2 = g360_db_ventas::config::load_config();
                cfg2.last_supabase_sync = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                let _ = g360_db_ventas::config::save_config(&cfg2);
            }
            let retention_msg = if cleaned > 0 { format!(" + {} antiguos limpiados", cleaned) } else { String::new() };
            set_state("idle", &format!("Sync OK: {} rows{}", up, retention_msg), 1.0);
            Ok(format!("Sync OK: {} rows (incremental, ventana: {} anos){}", up, retention, retention_msg))
        }
        Err(e) => {
            set_state("idle", &format!("Error sync: {}", e), 0.0);
            Err(format!("Error en upload: {}", e))
        }
    }
}

#[tauri::command]
async fn capture_range(startDate: String, endDate: String) -> Result<CaptureStatus, String> {
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
        if let Err(e) = g360_db_ventas::capture::run_batch_history(&sd, &ed, false, shared, Some(CAPTURE_ABORT.clone())).await {
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
async fn sync_from_last() -> Result<CaptureStatus, String> {
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
            let _ = std::fs::remove_file(&lock_path);
            if lock_path.exists() {
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
        if let Err(e) = g360_db_ventas::capture::run_batch_history(&last_date, "", false, shared, Some(CAPTURE_ABORT.clone())).await {
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

// ── Gestion de BD local ────────────────────────────────────────────────
#[tauri::command]
async fn clear_cache() -> Result<CaptureStatus, String> {
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
async fn clear_db() -> Result<CaptureStatus, String> {
    // Gate de seguridad: solo usuario con credenciales validas puede borrar
    match verify_intranet_credentials().await {
        Ok(_) => {},
        Err(e) => {
            return Err(format!("No autorizado: {}", e));
        }
    }
    // 1. Abortar captura en curso (flag global chequeado dentro de get_and_post_export)
    g360_db_ventas::capture::CAPTURE_ABORT_GLOBAL.store(true, Ordering::Relaxed);
    let _ = std::fs::remove_file(g360_db_ventas::config::raw_dir().join("capture.lock"));

    // 2. Esperar a que el task en vuelo suelte handles (GET/POST abortan en <=2s)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let db_path = g360_db_ventas::config::db_path();
    let db_dir = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));

    // Backup automatico antes de destruir (historial.db -> data/backup/historial_YYYYMMDD_HHMM.db)
    if db_path.exists() {
        let backup_dir = db_dir.join("backup");
        let _ = std::fs::create_dir_all(&backup_dir);
        let ts = now_secs();
        // yyyymmdd_hhmm desde epoch
        let secs_pe = ts.saturating_sub(5 * 3600);
        let days = (secs_pe / 86400) as i64;
        let rem = secs_pe % 86400;
        let (h, mi) = (rem / 3600, (rem % 3600) / 60);
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as i64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let mut y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 };
        if mo <= 2 { y += 1; }
        let fname = format!("historial_{:04}{:02}{:02}_{:02}{:02}.db", y, mo, d, h, mi);
        let backup_path = backup_dir.join(&fname);
        match std::fs::copy(&db_path, &backup_path) {
            Ok(_) => eprintln!("backup db -> {}", backup_path.display()),
            Err(e) => eprintln!("backup db fallo: {e}"),
        }
    }

    // Retry borrado (Windows: handle puede tardar en liberarse)
    for attempt in 1..=5 {
        match std::fs::remove_file(&db_path) {
            Ok(_) => break,
            Err(e) if attempt < 5 => {
                eprintln!("clear_db remove intento {attempt} fallo: {e}; reintentando...");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(_) => {} // continuar aunque no se pueda borrar (pool fresco lo recrea)
        }
    }
    for suffix in ["-wal", "-shm"] {
        let other = db_path.with_file_name(format!("{}{}", db_path.file_name().and_then(|n| n.to_str()).unwrap_or("historial"), suffix));
        let _ = std::fs::remove_file(&other);
    }
    let _ = std::fs::create_dir_all(db_dir);

    // Limpieza TOTAL local (como instalacion nueva): BD + archivos descargados + failed markers.
    // NO afecta en nada a Supabase.
    let raw = g360_db_ventas::config::raw_dir();
    if let Ok(entries) = std::fs::read_dir(&raw) {
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let is_data = name.starts_with("ventas_") && (name.ends_with(".csv") || name.ends_with(".xls") || name.ends_with(".html"));
            let is_summary = name.starts_with("summary");
            let is_failed_marker = name.starts_with("failed_") && name.ends_with(".json");
            if is_data || is_summary || is_failed_marker {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    // Reset flag para el proximo batch
    g360_db_ventas::capture::CAPTURE_ABORT_GLOBAL.store(false, Ordering::Relaxed);

    // Crear pool fresco (una sola vez)
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let msg = format!("BD local reiniciada ({count} regs). Solo afecto lo local - Supabase intacto. Defina fechas y ejecute una nueva captura.");
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
async fn get_capture_status() -> Result<CaptureStatus, String> {
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

#[tauri::command]
async fn list_months() -> Result<Vec<MonthStats>, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let months: Vec<MonthStats> = sqlx::query_as(
        "SELECT mes_ref, COUNT(*) as rows, COALESCE(SUM(soles), 0.0) as sales, COUNT(DISTINCT id_cliente) as clients FROM ventas GROUP BY mes_ref ORDER BY mes_ref"
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(months)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletenessInfo {
    pub mes_ref: String,
    pub dias_esperados: i32,
    pub dias_con_data: i32,
    pub dailies_en_disco: i32,
    pub filas: i64,
    pub faltan: Vec<String>,
    pub estado: String, // OK / PARCIAL / VACIO
}

#[tauri::command]
async fn get_completeness() -> Result<Vec<CompletenessInfo>, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let months: Vec<MonthStats> = sqlx::query_as("SELECT mes_ref, COUNT(*) as rows, COALESCE(SUM(soles),0) as sales, COUNT(DISTINCT id_cliente) as clients FROM ventas GROUP BY mes_ref ORDER BY mes_ref").fetch_all(&pool).await.map_err(|e| e.to_string())?;
    let raw = g360_db_ventas::config::raw_dir();
    let mut out = Vec::new();
    for m in months {
        let parts: Vec<&str> = m.mes_ref.split('-').collect();
        if parts.len()!=2 { continue; }
        let y: i32 = parts[0].parse().unwrap_or(2000);
        let mo: u32 = parts[1].parse().unwrap_or(1);
        let dias_esperados = {
            let is_leap = (y%4==0 && y%100!=0) || (y%400==0);
            match mo { 1|3|5|7|8|10|12=>31, 4|6|9|11=>30, 2=> if is_leap {29} else {28}, _=>30 }
        };
        // dias con data en BD (es la fuente de verdad: los CSV por-dia casi nunca existen porque capturamos por mes completo)
        let dias_con_data: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT fecha_orig) FROM ventas WHERE mes_ref=?").bind(&m.mes_ref).fetch_one(&pool).await.map_err(|e| e.to_string())?;
        // dias sin data en BD (para reportar faltantes reales)
        let mut faltan: Vec<String> = Vec::new();
        for d in 1..=dias_esperados {
            let ds = format!("{}-{:02}", m.mes_ref, d);
            let has: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref=? AND fecha_orig=?").bind(&m.mes_ref).bind(&ds).fetch_one(&pool).await.map_err(|e| e.to_string())?;
            if has == 0 { faltan.push(format!("{:02}", d)); }
        }
        // Estado por dias con data real: >=20 => OK (domingos/feriados no emiten ventas),
        // 1..19 => PARCIAL, 0 => VACIO. dailies_en_disco se conserva como info.
        let mut dailies = 0i32;
        for d in 1..=dias_esperados {
            let p = raw.join(format!("{}-{:02}.csv", m.mes_ref, d));
            if p.exists() && std::fs::metadata(&p).map(|mm| mm.len()>1000).unwrap_or(false) { dailies+=1; }
        }
        let estado = if dias_con_data==0 { "VACIO".to_string() }
            else if dias_con_data >= 20 { "OK".to_string() }
            else if dias_con_data >= 1 { "PARCIAL".to_string() }
            else { "PARCIAL".to_string() };
        out.push(CompletenessInfo{ mes_ref: m.mes_ref.clone(), dias_esperados, dias_con_data: dias_con_data as i32, dailies_en_disco: dailies, filas: m.rows, faltan: faltan.into_iter().take(5).collect(), estado });
    }
    Ok(out)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FailedDayInfo {
    pub mes: String,
    pub dias_faltantes: Vec<String>,
}

#[tauri::command]
async fn get_failed_days() -> Result<Vec<FailedDayInfo>, String> {
    let raw = g360_db_ventas::config::raw_dir();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&raw) {
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with("failed_") && name.ends_with(".json") {
                let mes = name.replace("failed_", "").replace(".json", "");
                if let Ok(s) = std::fs::read_to_string(&p) {
                    if let Ok(v) = serde_json::from_str::<Vec<String>>(&s) {
                        out.push(FailedDayInfo{ mes, dias_faltantes: v });
                    }
                }
            }
        }
    }
    Ok(out)
}

#[tauri::command]
async fn import_manual_day(day: String, xls_path: String) -> Result<String, String> {
    // day: YYYY-MM-DD, xls_path: ruta absoluta del XLS descargado manualmente
    if day.len()!=10 { return Err("Formato dia debe ser YYYY-MM-DD".into()); }
    let src = std::path::Path::new(&xls_path);
    if !src.exists() { return Err(format!("XLS no existe: {}", xls_path)); }
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw).map_err(|e| e.to_string())?;
    let dest_xls = raw.join(format!("ventas_{}.xls", day));
    std::fs::copy(src, &dest_xls).map_err(|e| e.to_string())?;
    // Convertir XLS -> CSV via calamine (Rust puro, sin python)
    let dest_csv = raw.join(format!("ventas_{}.csv", day));
    match g360_db_ventas::processor::xls::xls_to_csv(&dest_xls, &dest_csv) {
        Ok(n) if n > 0 => {},
        Ok(_) => return Err("CSV generado vacio (0 filas)".into()),
        Err(e) => return Err(format!("Conversion XLS->CSV fallo: {}", e)),
    }
    // Parsear e insertar en la BD (dedup protege contra duplicados)
    match g360_db_ventas::processor::parser::parse_export_csv(&dest_csv) {
        Ok(ventas) if !ventas.is_empty() => {
            let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
            let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas).await.map_err(|e| e.to_string())?;
            let _ = g360_db_ventas::db::writer::dedup_ventas(&pool).await;
            // Limpiar failed_*.json del mes
            let mes = &day[..7];
            let fail_path = raw.join(format!("failed_{}.json", mes));
            if fail_path.exists() {
                if let Ok(s) = std::fs::read_to_string(&fail_path) {
                    if let Ok(mut v) = serde_json::from_str::<Vec<String>>(&s) {
                        v.retain(|d| d != &day);
                        if v.is_empty() { let _ = std::fs::remove_file(&fail_path); }
                        else { let _ = std::fs::write(&fail_path, serde_json::to_string(&v).unwrap_or_default()); }
                    }
                }
            }
            Ok(format!("Dia {} importado e insertado: {} filas", day, n))
        }
        Ok(_) => Err("Parse devolvio 0 filas — verifica que el XLS sea del reporte Estadistica11".into()),
        Err(e) => Err(format!("Parse fallo: {}", e)),
    }
}

#[tauri::command]
async fn import_manual_month(month: String, xls_path: String) -> Result<String, String> {
    // month: YYYY-MM, xls_path: XLS mensual completo descargado manualmente del intranet
    if month.len()!=7 || !month.contains('-') { return Err("Formato mes debe ser YYYY-MM".into()); }
    let src = std::path::Path::new(&xls_path);
    if !src.exists() { return Err(format!("XLS no existe: {}", xls_path)); }
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw).map_err(|e| e.to_string())?;
    let dest_xls = raw.join(format!("ventas_{}.xls", month));
    std::fs::copy(src, &dest_xls).map_err(|e| e.to_string())?;
    // Convertir XLS -> CSV via calamine (Rust puro, sin python)
    let dest_csv = raw.join(format!("ventas_{}.csv", month));
    match g360_db_ventas::processor::xls::xls_to_csv(&dest_xls, &dest_csv) {
        Ok(n) if n > 0 => {},
        Ok(_) => return Err("CSV generado vacio (0 filas)".into()),
        Err(e) => return Err(format!("Conversion XLS->CSV fallo: {}", e)),
    }
    // Parsear + insertar (dedup final protege contra duplicados con dailies previos)
    match g360_db_ventas::processor::parser::parse_export_csv(&dest_csv) {
        Ok(ventas) if !ventas.is_empty() => {
            let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
            let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas).await.map_err(|e| e.to_string())?;
            let _ = g360_db_ventas::db::writer::dedup_ventas(&pool).await;
            // Limpiar failed json del mes
            let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", month)));
            Ok(format!("Mes {} importado desde XLS manual: {} filas insertadas (dedup aplicado)", month, n))
        }
        Ok(_) => Err("Parse devolvio 0 filas — verifica que el XLS sea del reporte Estadistica11".into()),
        Err(e) => Err(format!("Parse fallo: {}", e)),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreviewResult {
    pub file_name: String,
    pub total_rows: usize,
    pub insertable_rows: usize,
    pub tipo_desglose: std::collections::HashMap<String, usize>,
    pub referencia_status: std::collections::HashMap<String, usize>,
    pub preview_rows: Vec<Vec<String>>,
    pub warnings: Vec<String>,
}

#[tauri::command]
async fn preview_import(xls_path: String) -> Result<PreviewResult, String> {
    let src = std::path::Path::new(&xls_path);
    if !src.exists() { return Err(format!("XLS no existe: {}", xls_path)); }
    // Convertir a CSV temporal via calamine (no toca raw/)
    let tmp_csv = std::env::temp_dir().join(format!("preview_{}.csv", chrono::Utc::now().timestamp_millis()));
    g360_db_ventas::processor::xls::xls_to_csv(src, &tmp_csv).map_err(|e| e.to_string())?;
    let ventas = g360_db_ventas::processor::parser::parse_export_csv(&tmp_csv).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp_csv);
    let mut tipo_desglose: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for v in &ventas { *tipo_desglose.entry(v.tipo_operacion.clone()).or_insert(0) += 1; }
    // Clasificacion de referencias para NC/ND
    let mut ref_status: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let min_date = g360_db_ventas::config::MIN_AVAILABLE_DATE;
    for v in &ventas {
        if v.tpo_doc.contains("NCR") || v.tpo_doc.contains("NDB") {
            let s = if v.referencia.trim().is_empty() { "NO_ENCONTRADA".to_string() }
            else if v.factura_ref_serie.is_empty() || v.factura_ref_nro.is_empty() { "NO_ENCONTRADA".to_string() }
            else {
                let exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE serie_doc=? AND nro_doc=? AND (tpo_doc LIKE 'F01%' OR tpo_doc='F01')")
                    .bind(&v.factura_ref_serie).bind(&v.factura_ref_nro).fetch_one(&pool).await.unwrap_or(0) > 0;
                if exists { "REFERENCIA_OK".to_string() }
                else {
                    // Fuera de ventana si la fecha del NC es cercana al inicio disponible y la ref es de antes
                    let outside = v.fecha_orig < min_date || v.fecha_orig < min_date + chrono::Duration::days(60);
                    if outside { "DOCUMENTO_FUERA_DE_VENTANA".to_string() } else { "PENDIENTE_DE_ERP".to_string() }
                }
            };
            *ref_status.entry(s).or_insert(0) += 1;
        }
    }
    // Preview de 3 filas
    let mut preview_rows: Vec<Vec<String>> = Vec::new();
    if let Ok(reader) = csv::Reader::from_path(&src.with_extension("csv")) { let _ = reader; }
    // Re-leer el tmp_csv ya borrado no sirve; usar ventas para preview
    for v in ventas.iter().take(3) {
        preview_rows.push(vec![v.tpo_doc.clone(), v.serie_doc.clone(), v.nro_doc.clone(), v.id_articulo.clone(), format!("{:.2}", v.soles), v.fecha_orig.to_string()]);
    }
    let warnings = if ventas.is_empty() { vec!["Parse devolvio 0 filas — verifica que sea Estadistica11".to_string()] } else { vec![] };
    Ok(PreviewResult {
        file_name: src.file_name().unwrap_or_default().to_string_lossy().to_string(),
        total_rows: ventas.len() + ref_status.values().sum::<usize>(),
        insertable_rows: ventas.len(),
        tipo_desglose,
        referencia_status: ref_status,
        preview_rows,
        warnings,
    })
}

#[tauri::command]
async fn reparse_raw() -> Result<String, String> {
    let raw = g360_db_ventas::config::raw_dir();
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&raw) {
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with("ventas_") && name.ends_with(".csv") && !name.contains("-p") && name.len() == "ventas_YYYY-MM.csv".len() {
                files.push(p);
            }
        }
    }
    files.sort();
    if files.is_empty() { return Err("No hay CSVs en raw/ para reprocesar".into()); }

    // Ejecutar en background (como capture_range) para que el frontend pueda
    // hacer polling y mostrar progreso en vivo via get_capture_status.
    let shared: SharedProgress = CAPTURE_STATE.clone();
    let shared_err = shared.clone();
    tauri::async_runtime::spawn(async move {
        {
            let mut s = shared.lock().unwrap();
            s.set_start("Reprocesando raw");
            s.set_phase(CapturePhase::Downloading, "Reprocesando raw");
            s.update_progress(0.0, "Reprocesando raw...");
        }
        let total_files = files.len();
        let mut total = 0usize;
        for (i, csv) in files.iter().enumerate() {
            match g360_db_ventas::processor::parser::parse_export_csv_with_cross(csv, &pool).await {
                Ok(ventas) if !ventas.is_empty() => {
                    match g360_db_ventas::db::writer::insert_ventas(&pool, &ventas).await {
                        Ok(n) => total += n,
                        Err(e) => eprintln!("reparse insert {}: {}", csv.display(), e),
                    }
                }
                Ok(_) => {},
                Err(e) => eprintln!("reparse {}: {}", csv.display(), e),
            }
            let pct = (i + 1) as f32 / total_files as f32;
            {
                let mut st = shared.lock().unwrap();
                st.phase = CapturePhase::Downloading;
                st.message = format!("Reprocesando mes {}/{}", i + 1, total_files);
                st.progress = pct;
            }
            // Actualizar KPIs en vivo para que el GUI muestre progreso
            if (i % 5 == 0) || i + 1 == total_files {
                let _ = g360_db_ventas::db::writer::refresh_stats_cache(&pool).await;
            }
        }
        let _ = g360_db_ventas::db::writer::dedup_ventas(&pool).await;
        let _ = g360_db_ventas::db::writer::refresh_stats_cache(&pool).await;
        let mut st = shared_err.lock().unwrap();
        st.set_phase(CapturePhase::Done, &format!("Re-parse completado: {} filas", total));
        st.update_progress(1.0, &format!("Re-parse completado: {} filas", total));
    });

    Ok("Reprocesando raw...".to_string())
}

#[tauri::command]
async fn delete_month(month: String) -> Result<String, String> {
    // Gate de seguridad: solo usuario con credenciales validas puede eliminar
    if let Err(e) = verify_intranet_credentials().await {
        return Err(format!("No autorizado: {}", e));
    }
    // Verificar que el mes existe
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
        .bind(&month)
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
    if count == 0 {
        return Err(format!("Mes '{}' no encontrado en la BD", month));
    }

    // Eliminar de la BD
    sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
        .bind(&month)
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;

    // Eliminar CSV raw si existe
    let raw = g360_db_ventas::config::raw_dir();
    let csv = raw.join(format!("ventas_{}.csv", month));
    if csv.exists() {
        let _ = std::fs::remove_file(&csv);
    }

    // Eliminar XLS raw si existe
    let xls = raw.join(format!("ventas_{}.xls", month));
    if xls.exists() {
        let _ = std::fs::remove_file(&xls);
    }

    // VACUUM para recomprimir
    sqlx::query("VACUUM").execute(&pool).await.map_err(|e| e.to_string())?;

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(format!("Eliminados {} registros de {}. {} registros restantes.", count, month, remaining))
}

#[tauri::command]
async fn delete_months(months: Vec<String>) -> Result<String, String> {
    // Gate de seguridad: solo usuario con credenciales validas puede eliminar
    if let Err(e) = verify_intranet_credentials().await {
        return Err(format!("No autorizado: {}", e));
    }
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let mut total_deleted = 0i64;
    let raw = g360_db_ventas::config::raw_dir();

    for month in &months {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
            .bind(month)
            .fetch_one(&pool)
            .await
            .map_err(|e| e.to_string())?;

        if count > 0 {
            sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
                .bind(month)
                .execute(&pool)
                .await
                .map_err(|e| e.to_string())?;
            total_deleted += count;

            // Eliminar archivos raw
            let csv = raw.join(format!("ventas_{}.csv", month));
            let xls = raw.join(format!("ventas_{}.xls", month));
            let _ = std::fs::remove_file(&csv);
            let _ = std::fs::remove_file(&xls);
        }
    }

    // VACUUM
    sqlx::query("VACUUM").execute(&pool).await.map_err(|e| e.to_string())?;

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(format!("Eliminados {} registros de {} meses. {} registros restantes.", total_deleted, months.len(), remaining))
}

// ── Control y Preview ──────────────────────────────────────────────────
#[tauri::command]
async fn abort_capture() -> Result<String, String> {
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

#[tauri::command]
async fn test_intranet() -> Result<String, String> {
    match g360_db_ventas::browser::captor::preflight_checks() {
        Ok(()) => Ok("Conexion a intranet OK. Chrome detectado.".into()),
        Err(e) => Err(e),
    }
}

#[tauri::command]
async fn preview_csv() -> Result<Vec<Vec<String>>, String> {
    let raw = g360_db_ventas::config::raw_dir();
    // Buscar el CSV mas reciente
    let mut csv_files: Vec<_> = std::fs::read_dir(&raw)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("ventas_") && name.ends_with(".csv") && !name.contains("-p")
        })
        .collect();
    csv_files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    let latest = csv_files.first().ok_or("No hay CSVs en el directorio raw")?;
    let path = latest.path();
    let mut reader = csv::Reader::from_path(&path).map_err(|e| e.to_string())?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let mut rows = vec![headers];
    for (i, row) in reader.records().enumerate() {
        if i >= 2 { break; }
        let r = row.map_err(|e| e.to_string())?;
        rows.push(r.iter().map(|s| s.trim().to_string()).collect());
    }
    Ok(rows)
}

#[tauri::command]
async fn save_service_role_key(service_role_key: String) -> Result<String, String> {
    let mut cfg = g360_db_ventas::config::load_config();
    cfg.supabase.service_role_key = if service_role_key.trim().is_empty() {
        String::new()
    } else {
        service_role_key
    };
    g360_db_ventas::config::save_config(&cfg).map_err(|e| e.to_string())?;
    let configured = !cfg.supabase.service_role_key.is_empty();
    Ok(format!(
        "Service role key {} configurado",
        if configured { "ha sido" } else { "ha sido eliminado" }
    ))
}

// ─── AUTO-SYNC PARA TASK SCHEDULER ────────────────────────────────────────

/// Ejecuta el pipeline diario y sale. Usado por Task Scheduler.
async fn run_auto_sync_and_exit() -> ! {
    use chrono::{Local, TimeDelta};
    
    let cfg = g360_db_ventas::config::load_config();
    
    // Verificar configuración mínima
    if !cfg.supabase.is_configured() {
        eprintln!("ERROR: Supabase no configurado");
        std::process::exit(1);
    }
    if cfg.intranet.user.is_empty() || cfg.intranet.pass.is_empty() {
        eprintln!("ERROR: Credenciales de intranet no configuradas");
        std::process::exit(1);
    }
    
    println!("Iniciando sync automático...");
    
    let pool = match g360_db_ventas::db::writer::init_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: No se pudo conectar a la BD: {}", e);
            std::process::exit(1);
        }
    };
    
    // Paso 1: Capturar día anterior
    println!("Paso 1: Capturando datos del día anterior...");
    let yesterday = Local::now().naive_local().date() - TimeDelta::days(1);
    let yesterday_str = yesterday.to_string();
    
    let capture_result = g360_db_ventas::capture::run_batch_history(&yesterday_str, "", false, CAPTURE_STATE.clone(), None).await;
    
    match capture_result {
        Ok(_) => {
            println!("✓ Captura completada: {}", yesterday_str);
            
            // Paso 2: Verificar integridad
            println!("Paso 2: Verificando integridad...");
            if let Ok(issues) = g360_db_ventas::db::writer::verify_integrity(&pool).await {
                for issue in &issues {
                    if issue.contains("⚠") || issue.contains("ERROR") {
                        eprintln!("  {}", issue);
                    }
                }
            }
            
            // Paso 3: Upload con verificación
            println!("Paso 3: Subiendo a Supabase...");
            let dry_run = match g360_db_ventas::processor::uploader::dry_run_upload(
                &pool, 
                cfg.supabase_retention_days_effective(), 
                cfg.last_supabase_sync.as_deref()
            ).await {
                Ok(dr) => dr,
                Err(e) => {
                    eprintln!("ERROR: No se pudo calcular dry-run: {}", e);
                    std::process::exit(1);
                }
            };
            
            if dry_run.rows_to_upload == 0 {
                println!("No hay datos nuevos para subir.");
                std::process::exit(0);
            }
            
            println!("Subiendo {} registros...", dry_run.rows_to_upload);
            
            let shared: SharedProgress = CAPTURE_STATE.clone();
            let progress_cb: g360_db_ventas::processor::uploader::ProgressCb = Some(Arc::new(move |batch, total, pct, msg| {
                let mut s = shared.lock().unwrap();
                s.phase = CapturePhase::Uploading;
                s.message = msg.to_string();
                s.progress = pct;
                drop(s);
            }));
            
            match g360_db_ventas::processor::uploader::upload_all(
                &pool, 
                cfg.supabase_retention_days_effective(), 
                cfg.last_supabase_sync.as_deref(), 
                &progress_cb
            ).await {
                Ok((up, cleaned)) => {
                    println!("✓ Upload completado: {} filas subidas, {} limpiadas", up, cleaned);
                    
                    // Verificación post-upload
                    let url = g360_db_ventas::config::get_supabase_url();
                    let key = g360_db_ventas::config::get_supabase_service_key();
                    
                    if let Ok(verification) = g360_db_ventas::processor::uploader::verify_upload_result(&url, &key, up).await {
                        if verification.matched {
                            println!("✓ Verificación OK: {} filas confirmadas en Supabase", verification.actual_count);
                            
                            // Actualizar marker
                            let mut cfg2 = g360_db_ventas::config::load_config();
                            cfg2.last_supabase_sync = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                            let _ = g360_db_ventas::config::save_config(&cfg2);
                            
                            println!("✓ Marker actualizado");
                        } else {
                            eprintln!("⚠ Advertencia: Discrepancia verificada. Esperadas: {}, Encontradas: {}", 
                                verification.expected_count, verification.actual_count);
                        }
                    }
                    
                    // Calcular checksums
                    println!("Paso 4: Calculando checksums...");
                    if let Ok(_) = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool).await {
                        println!("✓ Checksums calculados");
                    }
                    
                    println!("\n=== SYNC AUTOMÁTICO COMPLETADO ===");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("ERROR: Fallo en upload: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("ERROR: Fallo en captura: {}", e);
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    // Verificar si se ejecuta desde Task Scheduler
    let is_task_scheduler = args.iter().any(|a| a == "--task-scheduler" || a == "--auto-sync");
    
    if is_task_scheduler {
        // Ejecutar sync y salir
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(run_auto_sync_and_exit());
    } else {
        // Modo normal: ejecutar app Tauri
        #[cfg(not(debug_assertions))]
        {
            use tracing_subscriber::{fmt::Layer, EnvFilter};
            use std::fs::OpenOptions;
            use std::io::Write;
            
            let log_dir = g360_db_ventas::config::logs_dir();
            let _ = std::fs::create_dir_all(&log_dir);
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(log_dir.join("app.log")) {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::new("info"))
                    .with_writer(std::sync::Mutex::new(f))
                    .with_ansi(false)
                    .init();
            }
        }
        
        tauri::Builder::default()
            .invoke_handler(tauri::generate_handler![
                get_dashboard, get_health, get_settings, save_settings,
                test_supabase, upload_all, admin_reset, reset_sync_marker, save_service_role_key,
                capture_range, sync_from_last,
                clear_cache, clear_db,
                get_capture_status,
                list_months, delete_month, delete_months, get_completeness, get_failed_days, import_manual_day, import_manual_month,
                preview_import, reparse_raw,
                abort_capture, test_intranet, preview_csv,
                // Auditoría e integridad
                verify_integrity, calculate_checksums, get_sync_history, get_checksum_history,
                // Protección de upload
                upload_dry_run, upload_all_with_verify,
                // Automatización diaria
                auto_daily_pipeline, get_next_capture_time
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }
}

// ─── COMANDOS DE AUDITORÍA E INTEGRIDAD ─────────────────────────────────────

#[tauri::command]
async fn verify_integrity() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::ensure_audit_tables(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let issues = g360_db_ventas::db::writer::verify_integrity(&pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(issues).unwrap_or(serde_json::json!({"error": "serialization failed"})))
}

#[tauri::command]
async fn calculate_checksums() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::ensure_audit_tables(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let checksums = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool)
        .await
        .map_err(|e| e.to_string())?;
    // Convert to JSON array of objects
    let mut result = Vec::new();
    for (mes, checksum, filas, soles) in checksums {
        result.push(serde_json::json!({
            "mes_ref": mes,
            "checksum": checksum,
            "total_filas": filas,
            "total_soles": soles
        }));
    }
    Ok(serde_json::to_value(result).unwrap_or(serde_json::json!({"error": "serialization failed"})))
}

#[tauri::command]
async fn get_sync_history(limit: i64) -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let history = g360_db_ventas::db::writer::get_sync_history(&pool, limit)
        .await
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for (id, tipo, estado, solicitadas, subidas, limpiadas, fecha) in history {
        result.push(serde_json::json!({
            "id": id,
            "tipo": tipo,
            "estado": estado,
            "filas_solicitadas": solicitadas,
            "filas_subidas": subidas,
            "filas_limpiadas": limpiadas,
            "started_at": fecha
        }));
    }
    Ok(serde_json::to_value(result).unwrap_or(serde_json::json!({"error": "serialization failed"})))
}

#[tauri::command]
async fn get_checksum_history() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let history = g360_db_ventas::db::writer::get_checksum_history(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for (mes, checksum, filas, soles, calculado_en) in history {
        result.push(serde_json::json!({
            "mes_ref": mes,
            "checksum": checksum,
            "total_filas": filas,
            "total_soles": soles,
            "calculado_en": calculado_en
        }));
    }
    Ok(serde_json::to_value(result).unwrap_or(serde_json::json!({"error": "serialization failed"})))
}

// ─── COMANDOS DE PROTECCIÓN PARA UPLOAD ─────────────────────────────────────

/// Dry-run: muestra qué se subiría sin hacer upload real
#[tauri::command]
async fn upload_dry_run() -> Result<serde_json::Value, String> {
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

/// Ejecuta el upload con verificación post-upload
#[tauri::command]
async fn upload_all_with_verify() -> Result<serde_json::Value, String> {
    let cfg = g360_db_ventas::config::load_config();
    if !cfg.supabase.is_configured() {
        return Err("Supabase no configurado".to_string());
    }

    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let retention = cfg.supabase_retention_days_effective();
    let last_sync = cfg.last_supabase_sync.clone();

    // Dry-run primero
    let dry_run = g360_db_ventas::processor::uploader::dry_run_upload(&pool, retention, last_sync.as_deref())
        .await
        .map_err(|e| e.to_string())?;

    let rows_to_upload = dry_run.rows_to_upload;

    if rows_to_upload == 0 {
        return Ok(serde_json::json!({
            "status": "idle",
            "message": "Nada nuevo para subir (sync incremental)",
            "rows_to_upload": 0,
            "verified": false
        }));
    }

    set_state("uploading", &format!("Subiendo {} registros a Supabase...", rows_to_upload), 0.05);

    let shared: SharedProgress = CAPTURE_STATE.clone();
    let progress_cb: g360_db_ventas::processor::uploader::ProgressCb = Some(Arc::new(move |_batch, _total, pct, msg| {
        let mut s = shared.lock().unwrap();
        s.phase = CapturePhase::Uploading;
        s.message = msg.to_string();
        s.progress = pct;
        s.started_at = s.started_at.or_else(|| Some(now_secs()));
        s.finished_at = None;
        drop(s);
    }));

    match g360_db_ventas::processor::uploader::upload_all(&pool, retention, last_sync.as_deref(), &progress_cb).await {
        Ok((up, cleaned)) => {
            // Verificación post-upload
            let url = g360_db_ventas::config::get_supabase_url();
            let key = g360_db_ventas::config::get_supabase_service_key();

            let verification = match g360_db_ventas::processor::uploader::verify_upload_result(&url, &key, up).await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Post-upload verification failed: {}", e);
                    g360_db_ventas::processor::uploader::VerificationResult {
                        expected_count: up,
                        actual_count: 0,
                        matched: false,
                        discrepancy: up,
                    }
                }
            };

            // Actualizar marcador solo si verificación pasó
            if up > 0 && verification.matched {
                let mut cfg2 = g360_db_ventas::config::load_config();
                cfg2.last_supabase_sync = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                let _ = g360_db_ventas::config::save_config(&cfg2);
            }

            let retention_msg = if cleaned > 0 { format!(" + {} antiguos limpiados", cleaned) } else { String::new() };
            let verify_msg = if verification.matched {
                format!(" verificado: {} filas confirmadas", verification.actual_count)
            } else {
                format!(" verificación: esperadas {}, encontradas {}", verification.expected_count, verification.actual_count)
            };

            set_state("idle", &format!("Sync OK: {} rows{}", up, retention_msg), 1.0);

            Ok(serde_json::json!({
                "status": "ok",
                "message": format!("Sync OK: {} rows{}{}", up, retention_msg, verify_msg),
                "rows_uploaded": up,
                "rows_cleaned": cleaned,
                "verification": {
                    "expected_count": verification.expected_count,
                    "actual_count": verification.actual_count,
                    "matched": verification.matched,
                    "discrepancy": verification.discrepancy
                },
                "marker_updated": up > 0 && verification.matched
            }))
        }
        Err(e) => {
            set_state("idle", &format!("Error sync: {}", e), 0.0);
            Err(format!("Error en upload: {}", e))
        }
    }
}

// ─── AUTOMATIZACIÓN DIARIA ───────────────────────────────────────────────

/// Pipeline diario automático: captura + parseo + validación + sync
#[tauri::command]
async fn auto_daily_pipeline() -> Result<serde_json::Value, String> {
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

    let capture_result = g360_db_ventas::capture::run_batch_history(&yesterday_str, "", false, CAPTURE_STATE.clone(), None).await;

    match capture_result {
        Ok(_) => {
            eprintln!("Captura diaria completada: {}", yesterday_str);

            // Paso 2: Verificar integridad
            set_state("syncing", "Verificando integridad...", 0.5);
            let integrity = g360_db_ventas::db::writer::verify_integrity(&pool).await.unwrap_or_default();

            // Paso 3: Calcular checksums
            set_state("syncing", "Calculando checksums...", 0.7);
            let _ = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool).await;

            // Paso 4: Dry-run upload
            set_state("uploading", "Verificando datos para subir...", 0.8);
            let dry_run = g360_db_ventas::processor::uploader::dry_run_upload(&pool, cfg.supabase_retention_days_effective(), cfg.last_supabase_sync.as_deref())
                .await
                .map_err(|e| e.to_string())?;

            if dry_run.rows_to_upload == 0 {
                set_state("idle", "No hay datos nuevos para subir", 1.0);
                return Ok(serde_json::json!({
                    "status": "idle",
                    "message": "No hay datos nuevos para subir",
                    "captured_date": yesterday_str,
                    "rows_to_upload": 0
                }));
            }

            // Paso 5: Upload con verificación
            set_state("uploading", &format!("Subiendo {} registros...", dry_run.rows_to_upload), 0.85);

            let shared: SharedProgress = CAPTURE_STATE.clone();
            let progress_cb: g360_db_ventas::processor::uploader::ProgressCb = Some(Arc::new(move |_batch, _total, pct, msg| {
                let mut s = shared.lock().unwrap();
                s.phase = CapturePhase::Uploading;
                s.message = msg.to_string();
                s.progress = pct;
                drop(s);
            }));

            match g360_db_ventas::processor::uploader::upload_all(&pool, cfg.supabase_retention_days_effective(), cfg.last_supabase_sync.as_deref(), &progress_cb).await {
                Ok((up, cleaned)) => {
                    let url = g360_db_ventas::config::get_supabase_url();
                    let key = g360_db_ventas::config::get_supabase_service_key();

                    let verification = g360_db_ventas::processor::uploader::verify_upload_result(&url, &key, up)
                        .await
                        .unwrap_or(g360_db_ventas::processor::uploader::VerificationResult {
                            expected_count: up,
                            actual_count: 0,
                            matched: false,
                            discrepancy: up,
                        });

                    if up > 0 && verification.matched {
                        let mut cfg2 = g360_db_ventas::config::load_config();
                        cfg2.last_supabase_sync = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                        let _ = g360_db_ventas::config::save_config(&cfg2);
                    }

                    set_state("idle", &format!("Pipeline diario completado: {} filas subidas", up), 1.0);

                    Ok(serde_json::json!({
                        "status": "ok",
                        "message": format!("Pipeline completado: {} capturado, {} filas subidas", yesterday_str, up),
                        "captured_date": yesterday_str,
                        "rows_uploaded": up,
                        "rows_cleaned": cleaned,
                        "marker_updated": up > 0 && verification.matched,
                        "integrity_issues": integrity.len()
                    }))
                }
                Err(e) => {
                    set_state("idle", &format!("Error en upload: {}", e), 0.0);
                    Err(format!("Error en upload: {}", e))
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
async fn get_next_capture_time() -> Result<serde_json::Value, String> {
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
