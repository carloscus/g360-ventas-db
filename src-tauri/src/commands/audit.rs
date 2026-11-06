


#[tauri::command]
pub async fn verify_integrity() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::ensure_audit_tables(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let issues = g360_db_ventas::db::writer::verify_integrity(&pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(issues).unwrap_or(serde_json::json!({"error": "serialization failed"})))
}

/// Reporte de calidad de datos (críticos / warnings / info).
#[tauri::command]
pub async fn get_data_quality() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::data_quality_report(&pool)
        .await
        .map_err(|e| e.to_string())
}

/// Exporta NCR y NDB a CSV separados (con vínculo a la factura referenciada).
#[tauri::command]
pub async fn export_nc_ndb(desde: String, hasta: String) -> Result<Vec<String>, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::export_nc_ndb(&pool, &desde, &hasta)
        .await
        .map_err(|e| e.to_string())
}

/// Monitor mensual: mes | filas | unidades | total soles (para comparar con la intranet).
#[tauri::command]
pub async fn get_monthly_monitor() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let rows = g360_db_ventas::db::writer::monthly_monitor(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| serde_json::json!({
            "mes_ref": r.mes_ref,
            "filas": r.filas,
            "total_soles": r.total_soles,
            "clientes": r.clientes
        }))
        .collect();
    Ok(serde_json::Value::Array(arr))
}

/// Exporta el monitor mensual a XLSX.
#[tauri::command]
pub async fn export_monitor_xlsx() -> Result<String, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::export_monitor_xlsx(&pool)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn calculate_checksums() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::ensure_audit_tables(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let checksums = g360_db_ventas::db::writer::calculate_monthly_checksums(&pool)
        .await
        .map_err(|e| e.to_string())?;
    // checksums es un usize (count de meses calculados)
    Ok(serde_json::json!({ "meses_calculados": checksums }))
}

#[tauri::command]
pub async fn get_sync_history(limit: i64) -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let history = g360_db_ventas::db::writer::get_sync_history(&pool, limit)
        .await
        .map_err(|e| e.to_string())?;
    // history es Vec<serde_json::Value> — ya viene serializado
    Ok(serde_json::Value::Array(history))
}

#[tauri::command]
pub async fn get_checksum_history() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let history = g360_db_ventas::db::writer::get_checksum_history(&pool)
        .await
        .map_err(|e| e.to_string())?;
    // history es Vec<serde_json::Value> — ya viene serializado
    Ok(serde_json::Value::Array(history))
}

// ─── COMANDOS DE PROTECCIÓN PARA UPLOAD ─────────────────────────────────────

/// Calcula checksums para meses que aun no tienen hash (complementa los 105/121).
#[tauri::command]
pub async fn recalculate_checksums() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let n = g360_db_ventas::db::writer::recalculate_missing_checksums(&pool)
        .await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "meses_calculados": n }))
}

