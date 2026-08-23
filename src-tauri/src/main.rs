#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::{Mutex, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

static CAPTURE_STATE: LazyLock<Mutex<CaptureStatus>> = LazyLock::new(|| Mutex::new(CaptureStatus { state: "idle".into(), message: "Listo".into(), progress: 0.0, started_at: None, finished_at: None }));

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
    pub message: String,
    pub progress: f32,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
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
    pub supabase_configured: bool,
    pub db_path: String,
}

fn set_state(state: &str, msg: &str, progress: f32) {
    let mut s = CAPTURE_STATE.lock().unwrap();
    s.state = state.into();
    s.message = msg.into();
    s.progress = progress;
    if state == "syncing" || state == "uploading" {
        s.started_at = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
        s.finished_at = None;
    } else if state == "idle" {
        s.finished_at = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
    }
}

#[tauri::command]
async fn get_dashboard() -> Result<DashboardStats, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let total_records: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let total_sales: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(soles), 0.0) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let total_clients: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id_cliente) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let total_skus: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT original_sku) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let months: Vec<MonthStats> = sqlx::query_as("SELECT mes_ref, COUNT(*) as rows, COALESCE(SUM(soles), 0.0) as sales, COUNT(DISTINCT id_cliente) as clients FROM ventas GROUP BY mes_ref ORDER BY mes_ref").fetch_all(&pool).await.map_err(|e| e.to_string())?;
    Ok(DashboardStats { total_records, total_sales, total_clients, total_skus, months })
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
    let last_capture = std::fs::metadata(&db_path).and_then(|m| m.modified()).map(|t| format!("{:?}", t)).unwrap_or("n/a".into());
    let last_capture = last_capture.chars().take(19).collect::<String>();
    let status = if total_records > 700000 { "OK".to_string() } else if total_records > 0 { "SYNCING".to_string() } else { "EMPTY".to_string() };
    Ok(HealthStats { total_records, min_date: min_date.unwrap_or_default(), max_date: max_date.unwrap_or_default(), last_capture, db_size_mb, status, db_path: db_path.to_string_lossy().to_string(), db_exists })
}

#[tauri::command]
async fn check_db() -> Result<String, String> {
    let db_path = g360_db_ventas::config::db_path();
    let exists = db_path.exists();
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let dir = g360_db_ventas::config::data_dir();
    let dir_exists = dir.exists();
    Ok(format!("DB exists: {}, path: {}, size: {} bytes, dir exists: {}", exists, db_path.display(), size, dir_exists))
}

#[tauri::command]
async fn get_settings() -> Result<AppSettings, String> {
    let cfg = g360_db_ventas::config::load_config();
    let db_path = g360_db_ventas::config::db_path().to_string_lossy().to_string();
    Ok(AppSettings {
        supabase_url: cfg.supabase.url.clone(),
        supabase_key: cfg.supabase.key.clone(),
        supabase_configured: cfg.supabase.is_configured(),
        db_path,
    })
}

#[tauri::command]
async fn save_settings(supabase_url: String, supabase_key: String) -> Result<AppSettings, String> {
    let mut cfg = g360_db_ventas::config::load_config();
    cfg.supabase.url = supabase_url;
    cfg.supabase.key = supabase_key;
    g360_db_ventas::config::save_config(&cfg).map_err(|e| e.to_string())?;
    let db_path = g360_db_ventas::config::db_path().to_string_lossy().to_string();
    Ok(AppSettings {
        supabase_url: cfg.supabase.url.clone(),
        supabase_key: cfg.supabase.key.clone(),
        supabase_configured: cfg.supabase.is_configured(),
        db_path,
    })
}

#[tauri::command]
async fn test_supabase(url: String, key: String) -> Result<String, String> {
    let res = g360_db_ventas::processor::uploader::test_supabase_connection(&url, &key).await.map_err(|e| e.to_string())?;
    Ok(res)
}

#[tauri::command]
async fn upload_all() -> Result<String, String> {
    let cfg = g360_db_ventas::config::load_config();
    if !cfg.supabase.is_configured() {
        return Err("Supabase no configurado. Abra Configuracion e ingrese credenciales.".to_string());
    }
    set_state("uploading", "Conectando a Supabase...", 0.1);

    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    if count == 0 {
        set_state("idle", "BD vacia - nada que subir", 1.0);
        return Ok("BD vacia".to_string());
    }
    set_state("uploading", &format!("Leyendo {} registros...", count), 0.2);

    let ventas = g360_db_ventas::db::writer::fetch_all_ventas(&pool).await.map_err(|e| e.to_string())?;
    set_state("uploading", &format!("Subiendo {} registros a Supabase...", ventas.len()), 0.3);

    match g360_db_ventas::processor::uploader::upload_to_supabase(&ventas).await {
        Ok(n) => {
            set_state("idle", &format!("Subido OK: {} rows", n), 1.0);
            Ok(format!("Subido OK: {} rows a Supabase", n))
        }
        Err(e) => {
            set_state("idle", &format!("Error: {}", e), 1.0);
            Err(format!("Error: {}", e))
        }
    }
}

#[tauri::command]
async fn capture_range(startDate: String, endDate: String) -> Result<CaptureStatus, String> {
    set_state("syncing", &format!("Rango {} -> {} iniciado", startDate, endDate), 0.1);
    let sd = startDate.clone();
    let ed = endDate.clone();
    tauri::async_runtime::spawn(async move {
        let _ = g360_db_ventas::capture::run_batch_history(&sd, &ed, false).await;
    });
    Ok(CAPTURE_STATE.lock().unwrap().clone())
}

#[tauri::command]
async fn sync_from_last() -> Result<CaptureStatus, String> {
    set_state("syncing", "Sync desde ultima captura", 0.1);
    tauri::async_runtime::spawn(async move {
        let _ = g360_db_ventas::capture::run_batch_history("", "", true).await;
    });
    Ok(CAPTURE_STATE.lock().unwrap().clone())
}

#[tauri::command]
async fn clear_cache() -> Result<CaptureStatus, String> {
    let raw = g360_db_ventas::config::raw_dir();
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
    set_state("idle", &format!("Cache limpiada: {} temporales", removed), 1.0);
    Ok(CAPTURE_STATE.lock().unwrap().clone())
}

#[tauri::command]
async fn clear_db() -> Result<CaptureStatus, String> {
    let db_path = g360_db_ventas::config::db_path();
    let db_dir = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let _ = std::fs::remove_file(&db_path);
    for suffix in ["-wal", "-shm"] {
        let other = db_path.with_file_name(format!("{}{}", db_path.file_name().and_then(|n| n.to_str()).unwrap_or("historial"), suffix));
        let _ = std::fs::remove_file(&other);
    }
    let _ = std::fs::create_dir_all(db_dir);
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    if count == 0 {
        set_state("idle", "BD recreada: 0 registros", 1.0);
    } else {
        set_state("idle", &format!("BD recreada: {} registros", count), 1.0);
    }
    Ok(CAPTURE_STATE.lock().unwrap().clone())
}

#[tauri::command]
async fn get_capture_status() -> Result<CaptureStatus, String> {
    Ok(CAPTURE_STATE.lock().unwrap().clone())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_dashboard, get_health, check_db, get_settings, save_settings, test_supabase, upload_all, capture_range, sync_from_last, clear_cache, clear_db, get_capture_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
