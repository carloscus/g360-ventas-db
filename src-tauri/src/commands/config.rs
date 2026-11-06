use std::time::Instant;


use crate::state::*;
use crate::types::*;
use super::verify_intranet_credentials;

// ── Settings y Supabase ────────────────────────────────────────────────
#[tauri::command]
pub async fn get_settings() -> Result<AppSettings, String> {
    let cfg = g360_db_ventas::config::load_config();
    let db_path = g360_db_ventas::config::db_path().to_string_lossy().to_string();
    let sb_retention_days_eff = cfg.supabase_retention_days_effective();
    Ok(AppSettings {
        supabase_url: cfg.supabase.url.clone(),
        supabase_key: cfg.supabase.key.clone(),
        supabase_service_role_configured: !cfg.supabase.service_role_key.is_empty(),
        supabase_table: g360_db_ventas::config::SUPABASE_TABLE.to_string(),
        supabase_configured: cfg.supabase.is_configured(),
        supabase_enabled: cfg.supabase_enabled,
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
        backup_reminder_days: cfg.backup_reminder_days,
    })
}

#[tauri::command]
pub async fn save_settings(
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
    supabase_enabled: Option<bool>,
    backup_reminder_days: Option<u32>,
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
     if let Some(en) = supabase_enabled {
         cfg.supabase_enabled = en;
     }
     if !capture_times.is_empty() {
         cfg.capture_times = capture_times;
     }
     if let Some(d) = backup_reminder_days {
         cfg.backup_reminder_days = d;
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
        supabase_enabled: cfg.supabase_enabled,
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
        backup_reminder_days: cfg.backup_reminder_days,
    })
}

#[tauri::command]
pub async fn test_supabase(url: String, key: String) -> Result<String, String> {
    let res = g360_db_ventas::processor::uploader::test_supabase_connection(&url, &key).await.map_err(|e| e.to_string())?;
    Ok(res)
}

/// Cobertura de Supabase: fecha mas antigua, mas nueva, filas y documentos.
#[tauri::command]
pub async fn get_supabase_info() -> Result<g360_db_ventas::processor::uploader::SupabaseInfo, String> {
    {
        let c = SUPABASE_INFO_CACHE.lock().unwrap();
        if let Some((t, info)) = c.as_ref() {
            if t.elapsed().as_secs() < SUPABASE_INFO_TTL_SECS {
                return Ok(info.clone());
            }
        }
    }
    let info = g360_db_ventas::processor::uploader::get_supabase_info()
        .await
        .map_err(|e| e.to_string())?;
    *SUPABASE_INFO_CACHE.lock().unwrap() = Some((Instant::now(), info.clone()));
    Ok(info)
}

#[tauri::command]
pub async fn test_intranet() -> Result<String, String> {
    // Verificacion REAL: presencia + Chrome + login contra el intranet.
    // (Antes solo validaba presencia y podia reportar "OK" con credenciales invalidas.)
    if let Err(e) = g360_db_ventas::browser::captor::preflight_checks() {
        return Err(e);
    }
    verify_intranet_credentials().await?;
    Ok("Credenciales válidas · Chrome detectado".into())
}

/// Estado (barato, sin red) de las credenciales configuradas, para el arranque.
#[tauri::command]
pub async fn get_credentials_status() -> Result<serde_json::Value, String> {
    use g360_db_ventas::config::{effective_intranet_pass, effective_intranet_user};
    let cfg = g360_db_ventas::config::load_config();
    let user = effective_intranet_user();
    let pass = effective_intranet_pass();
    Ok(serde_json::json!({
        "intranet_configured": !user.is_empty() && !pass.is_empty(),
        "intranet_user": user,
        "supabase_configured": cfg.supabase.is_configured(),
        "supabase_enabled": cfg.supabase_enabled,
    }))
}

#[tauri::command]
pub async fn save_service_role_key(service_role_key: String) -> Result<String, String> {
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

/// Indica si es el primer inicio de la app (no hay config guardada).
#[tauri::command]
pub fn is_first_run() -> bool {
    g360_db_ventas::config::is_first_run()
}

/// Guarda la configuración inicial desde el wizard (primer inicio en PC nueva).
/// Encripta con DPAPI de esta PC/usuario.
#[tauri::command]
pub fn save_first_config(
    supabase_url: String,
    supabase_key: String,
    service_role_key: String,
    intranet_user: String,
    intranet_pass: String,
    allowed_lines: Vec<String>,
    retention_days: u32,
) -> Result<serde_json::Value, String> {
    let mut cfg = g360_db_ventas::config::AppConfig::default_app();
    cfg.supabase.url = supabase_url;
    cfg.supabase.key = supabase_key;
    cfg.supabase.service_role_key = service_role_key;
    cfg.intranet.user = intranet_user;
    cfg.intranet.pass = intranet_pass;
    if !allowed_lines.is_empty() {
        cfg.allowed_lines = allowed_lines;
    }
    cfg.supabase_retention_days = retention_days;
    cfg.supabase_retention_years = 0;
    cfg.generado_por = "wizard-primer-inicio".to_string();

    g360_db_ventas::config::save_config(&cfg).map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "ok": true,
        "message": "Configuración guardada encriptada (DPAPI)",
        "first_run": false
    }))
}

