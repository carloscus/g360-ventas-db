use std::sync::atomic::Ordering;

use g360_db_ventas::capture_state::now_secs;

use crate::state::*;
use crate::types::*;
use super::verify_intranet_credentials;

/// Reset admin: backup de config actual, genera config limpio con credenciales vacías.
/// Para intervención de tercero (admin/support) que necesita reconfigurar acceso.
#[tauri::command]
pub async fn admin_reset() -> Result<String, String> {
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

/// Abre la carpeta de datos local para copiarla a un extraíble o compartirla
/// mediante las herramientas de Windows. No crea recursos SMB ni expone la BD
/// automáticamente en la red.
#[tauri::command]
pub fn open_data_folder() -> Result<String, String> {
    let data_dir = g360_db_ventas::config::data_dir();
    std::fs::create_dir_all(&data_dir).map_err(|e| {
        format!("No se pudo preparar la carpeta de datos: {e}")
    })?;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| format!("No se pudo abrir la carpeta de datos: {e}"))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        return Err("Abrir la carpeta de datos solo está disponible en Windows".to_string());
    }

    Ok(data_dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn clear_db() -> Result<CaptureStatus, String> {
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

    // Liberar el archivo ANTES de borrar: cerrar el pool singleton. En Windows un
    // archivo abierto no se puede eliminar, y si el pool sigue vivo init_pool()
    // devuelve el mismo pool (con los datos) -> la BD "no se limpiaba".
    g360_db_ventas::db::writer::reset_pool().await;

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
    // Verificar que de verdad se borro (si el archivo sigue, abortar con mensaje claro).
    if db_path.exists() {
        return Err(
            "No se pudo borrar historial.db (archivo en uso). Cierra la app y reintenta.".into(),
        );
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

/// Ejecuta VACUUM para recomprimir la BD. Tarda 30-120s en BD de ~1GB.
#[tauri::command]
pub async fn vacuum_db() -> Result<String, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::vacuum_db(&pool).await.map_err(|e| e.to_string())?;
    Ok("VACUUM completado".to_string())
}

/// Normaliza un ID de entrada del usuario para búsquedas en la BD.
/// Cliente: zfill(8) → "10214" se convierte en "00010214"
/// Linea/Vendedor: lstrip('01') → "0178" se convierte en "78"
/// Articulo: sin transformar (clave natural)
#[tauri::command]
pub fn normalize_query_id_cmd(input: String, id_type: String) -> String {
    use g360_db_ventas::processor::parser::{normalize_query_id, IdType};
    let t = match id_type.to_lowercase().as_str() {
        "cliente" => IdType::Cliente,
        "linea" => IdType::Linea,
        "vendedor" => IdType::Vendedor,
        _ => IdType::Articulo,
    };
    normalize_query_id(&input, t)
}

const SCHED_TASK_NAME: &str = "G360-Actualizar-Ventas";

/// Consulta si la tarea diaria de actualización existe en Windows Task Scheduler.
#[tauri::command]
pub fn get_scheduled_task() -> serde_json::Value {
    let out = std::process::Command::new("schtasks")
        .args(["/query", "/tn", SCHED_TASK_NAME, "/fo", "LIST"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let time = text
                .lines()
                .find(|l| l.contains("Next Run Time") || l.contains("Hora"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
                .unwrap_or_default();
            serde_json::json!({ "exists": true, "next_run": time })
        }
        _ => serde_json::json!({ "exists": false, "next_run": null }),
    }
}

/// Crea/actualiza la tarea diaria de actualización completa (--task-scheduler).
/// `time` formato "HH:MM" (local).
#[tauri::command]
pub fn create_scheduled_task(time: String) -> Result<serde_json::Value, String> {
    let t = time.trim();
    let parts: Vec<&str> = t.split(':').collect();
    let ok = parts.len() == 2
        && parts[0].len() == 2
        && parts[1].len() == 2
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1].chars().all(|c| c.is_ascii_digit());
    if !ok {
        return Err(format!("Formato de hora inválido (esperado HH:MM): {}", time));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tr = format!("\"{}\" --task-scheduler", exe.display());
    let out = std::process::Command::new("schtasks")
        .args([
            "/create", "/tn", SCHED_TASK_NAME, "/sc", "daily", "/st", t, "/tr", &tr, "/f",
        ])
        .output()
        .map_err(|e| format!("No se pudo ejecutar schtasks: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "schtasks falló: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    eprintln!("Tarea programada creada: {} diario a las {}", SCHED_TASK_NAME, t);
    Ok(serde_json::json!({
        "ok": true,
        "message": format!("Actualización automática programada: diario a las {}", t),
        "command": tr
    }))
}

/// Elimina la tarea diaria de actualización.
#[tauri::command]
pub fn delete_scheduled_task() -> Result<serde_json::Value, String> {
    let out = std::process::Command::new("schtasks")
        .args(["/delete", "/tn", SCHED_TASK_NAME, "/f"])
        .output()
        .map_err(|e| format!("No se pudo ejecutar schtasks: {}", e))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.to_lowercase().contains("cannot find") || err.contains("no existe") {
            return Ok(serde_json::json!({ "ok": true, "message": "La tarea no existía" }));
        }
        return Err(format!("schtasks falló: {}", err.trim()));
    }
    eprintln!("Tarea programada eliminada: {}", SCHED_TASK_NAME);
    Ok(serde_json::json!({ "ok": true, "message": "Tarea de actualización automática eliminada" }))
}

