


/// Crea backup con fecha/hora de historial.db. Conserva ultimas 7 copias.
#[tauri::command]
pub async fn backup_db() -> Result<String, String> {
    g360_db_ventas::db::writer::backup_db().await.map_err(|e| e.to_string())
}

/// Exporta la BD completa a un archivo standalone (VACUUM INTO) listo para compartir.
/// Pobla dims y recalcula checksums antes de exportar.
#[tauri::command]
pub async fn export_full_db(path: String) -> Result<String, String> {
    let target = std::path::Path::new(&path);
    g360_db_ventas::db::writer::export_full_db(target).await.map_err(|e| e.to_string())
}

/// Exporta una instantánea canónica (snapshot .db + manifiesto JSON) a la carpeta
/// `data/export/`. Para que otra app trabaje sobre una copia aislada.
/// Retorna: db_path, manifest_path, sha256, contracto_version, data_quality, etc.
#[tauri::command]
pub async fn export_canonical_snapshot() -> Result<serde_json::Value, String> {
    let dir = g360_db_ventas::config::data_dir().join("export");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let target = dir.join(format!("base_canonica_{}.db", ts));
    // Sin stats de parseo (es un export desde la DB existente)
    g360_db_ventas::db::writer::export_canonical_snapshot(&target, None)
        .await
        .map_err(|e| e.to_string())
}

/// Verifica si la DB actual cumple con el contrato esperado.
/// Retorna: ok: bool, expected_version: i64, stored_version: Option<i64>, message: String
#[tauri::command]
pub async fn check_contract_version() -> Result<serde_json::Value, String> {
    use g360_db_ventas::db::writer::check_contract_version;
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    match check_contract_version(&pool).await {
        Ok(_) => Ok(serde_json::json!({
            "ok": true,
            "message": format!("Versión de contrato compatible (v{})", g360_db_ventas::db::schema::CONTRACTO_VERSION)
        })),
        Err(e) => Ok(serde_json::json!({
            "ok": false,
            "message": e.to_string()
        }))
    }
}

/// Retorna la metadata de versiones del contrato almacenada en g360_meta.
#[tauri::command]
pub async fn get_contract_metadata() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let contract_ver: String = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'contracto_version'"
    ).fetch_one(&pool).await.unwrap_or(g360_db_ventas::db::schema::CONTRACTO_VERSION.to_string());
    let dim_ver: String = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'dim_schema_version'"
    ).fetch_one(&pool).await.unwrap_or(g360_db_ventas::db::schema::DIM_SCHEMA_VERSION.to_string());
    let norm_ver: String = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'normalizacion_version'"
    ).fetch_one(&pool).await.unwrap_or(g360_db_ventas::db::schema::NORMALIZACION_VERSION.to_string());
    Ok(serde_json::json!({
        "contracto_version": contract_ver,
        "dim_schema_version": dim_ver,
        "normalizacion_version": norm_ver,
        "contracto_esperado": g360_db_ventas::db::schema::CONTRACTO_VERSION.to_string(),
        "dim_esperado": g360_db_ventas::db::schema::DIM_SCHEMA_VERSION.to_string(),
        "norm_esperado": g360_db_ventas::db::schema::NORMALIZACION_VERSION.to_string()
    }))
}

/// Retorna info del último cartucho generado (lee el .manifest.json más reciente).
#[tauri::command]
pub fn get_last_cartucho() -> Result<serde_json::Value, String> {
    let export_dir = g360_db_ventas::config::export_dir();
    if !export_dir.exists() {
        return Ok(serde_json::json!({ "exists": false }));
    }

    // Buscar el .manifest.json más reciente
    let mut manifests: Vec<std::path::PathBuf> = std::fs::read_dir(&export_dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| p.file_name().map(|x| x.to_string_lossy().contains("manifest")).unwrap_or(false))
        .collect();
    manifests.sort();

    match manifests.last() {
        Some(path) => {
            let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let manifest: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;

            // Verificar contrato contra BD actual
            let db_version = g360_db_ventas::db::schema::CONTRACTO_VERSION.to_string();
            let manifest_version = manifest.get("contracto_version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let synced = manifest_version == db_version;

            Ok(serde_json::json!({
                "exists": true,
                "path": path.to_string_lossy(),
                "manifest": manifest,
                "contracto_synced": synced,
                "db_contracto_version": db_version,
                "manifest_contracto_version": manifest_version
            }))
        }
        None => Ok(serde_json::json!({ "exists": false })),
    }
}

/// Comprime el último cartucho (.db + manifest) en un ZIP para transporte fácil.
#[tauri::command]
pub fn zip_cartucho_cmd() -> Result<serde_json::Value, String> {
    let (zip_path, db_bytes, zip_bytes) =
        g360_db_ventas::db::writer::zip_cartucho().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "zip_path": zip_path,
        "db_mb": (db_bytes as f64 / 1048576.0).round(),
        "zip_mb": (zip_bytes as f64 / 1048576.0).round(),
        "ratio_pct": if db_bytes > 0 { zip_bytes * 100 / db_bytes  } else { 0 },
        "message": format!(
            "ZIP creado ({:.0}% del original): {}",
            zip_bytes as f64 * 100.0 / db_bytes.max(1) as f64,
            zip_path
        )
    }))
}

/// Estado del recordatorio de backup local (basado en el backup más reciente).
#[tauri::command]
pub fn get_backup_status() -> serde_json::Value {
    let days = g360_db_ventas::config::load_config().backup_reminder_days;
    g360_db_ventas::db::writer::backup_status(days)
}

// ─── TAREA PROGRAMADA (Windows Task Scheduler) ──────────────────────────────

