/// Abre la carpeta de datos local para copiarla a un extraíble o compartirla
/// mediante las herramientas de Windows. No crea recursos SMB ni expone la BD
/// automáticamente en la red.
#[tauri::command]
pub fn open_data_folder() -> Result<String, String> {
    let data_dir = g360_db_ventas::config::data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("No se pudo preparar la carpeta de datos: {e}"))?;

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
            // split_once en el PRIMER ':' conserva la fecha completa:
            // "Next Run Time: 06/11/2026 14:00:00" -> "06/11/2026 14:00:00".
            // (split(':').nth(1) cortaba en el ':' de la hora y devolvia
            // "06/11/2026 14", sin minutos.)
            let time = text
                .lines()
                .find(|l| l.contains("Next Run Time") || l.contains("Hora"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, rest)| rest.trim().to_string())
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
        return Err(format!(
            "Formato de hora inválido (esperado HH:MM): {}",
            time
        ));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tr = format!("\"{}\" --task-scheduler", exe.display());
    let out = std::process::Command::new("schtasks")
        .args([
            "/create",
            "/tn",
            SCHED_TASK_NAME,
            "/sc",
            "daily",
            "/st",
            t,
            "/tr",
            &tr,
            "/f",
        ])
        .output()
        .map_err(|e| format!("No se pudo ejecutar schtasks: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "schtasks falló: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    eprintln!(
        "Tarea programada creada: {} diario a las {}",
        SCHED_TASK_NAME, t
    );
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
