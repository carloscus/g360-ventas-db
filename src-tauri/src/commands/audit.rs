/// Reporte de calidad de datos (críticos / warnings / info).
#[tauri::command]
pub async fn get_data_quality() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool()
        .await
        .map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::data_quality_report(&pool)
        .await
        .map_err(|e| e.to_string())
}

/// Exporta NCR y NDB a CSV separados (con vínculo a la factura referenciada).
#[tauri::command]
pub async fn export_nc_ndb(desde: String, hasta: String) -> Result<Vec<String>, String> {
    let pool = g360_db_ventas::db::writer::init_pool()
        .await
        .map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::export_nc_ndb(&pool, &desde, &hasta)
        .await
        .map_err(|e| e.to_string())
}

/// Monitor mensual: mes | filas | unidades | total soles (para comparar con la intranet).
#[tauri::command]
pub async fn get_monthly_monitor() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool()
        .await
        .map_err(|e| e.to_string())?;
    let rows = g360_db_ventas::db::writer::monthly_monitor(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "mes_ref": r.mes_ref,
                "filas": r.filas,
                "total_soles": r.total_soles,
                "clientes": r.clientes
            })
        })
        .collect();
    Ok(serde_json::Value::Array(arr))
}

/// Exporta el monitor mensual a XLSX.
#[tauri::command]
pub async fn export_monitor_xlsx() -> Result<String, String> {
    let pool = g360_db_ventas::db::writer::init_pool()
        .await
        .map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::export_monitor_xlsx(&pool)
        .await
        .map_err(|e| e.to_string())
}
