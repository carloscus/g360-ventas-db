
use g360_db_ventas::capture_state::{CapturePhase, SharedProgress};

use crate::state::*;
use crate::types::*;
use super::verify_intranet_credentials;

#[tauri::command]
pub async fn list_months() -> Result<Vec<MonthStats>, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let months: Vec<MonthStats> = sqlx::query_as(
        "SELECT mes_ref, COUNT(*) as rows, COALESCE(SUM(soles), 0.0) as sales, COUNT(DISTINCT id_cliente) as clients FROM ventas GROUP BY mes_ref ORDER BY mes_ref"
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(months)
}

#[tauri::command]
pub async fn get_completeness() -> Result<Vec<CompletenessInfo>, String> {
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

#[tauri::command]
pub async fn get_failed_days() -> Result<Vec<FailedDayInfo>, String> {
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

/// Gate de integridad para importacion manual: rechaza XLS cuya columna SKU
/// (ID_ARTICULO) este guardada como numero. En ese caso Excel pudo borrar los
/// ceros a la izquierda (011019 -> 11019) y la importacion crearia SKUs falsos
/// que colisionan con productos de otra linea. La captura automatica descarga
/// el XLS original del intranet (texto), por eso solo se valida al importar a mano.
pub fn reject_numeric_sku(xls_path: &str) -> Result<(), String> {
    match g360_db_ventas::processor::xls::inspect_sku_column(std::path::Path::new(xls_path)) {
        Ok(insp) if insp.has_numeric_sku() => {
            let n = insp.numeric_cells;
            let total = insp.total_cells;
            Err(format!(
                "⛔ Importación bloqueada: la columna SKU está guardada como número en {n} de {total} filas. \
                 Excel pudo eliminar ceros a la izquierda (011019 → 11019) y crear SKUs falsos. \
                 Vuelve a exportar/descargar el XLS del intranet SIN abrirlo ni re-guardarlo en Excel. \
                 Detalle: {}",
                insp.warning_message()
            ))
        }
        Ok(_) => Ok(()),
        Err(e) => {
            // Si no se pudo inspeccionar (formato raro), no bloquear: el parseo
            // posterior validara el contenido. Solo se registra.
            eprintln!("inspect_sku_column no concluyente para {}: {}", xls_path, e);
            Ok(())
        }
    }
}

#[tauri::command]
pub async fn import_manual_day(day: String, xls_path: String) -> Result<String, String> {
    // day: YYYY-MM-DD, xls_path: ruta absoluta del XLS descargado manualmente
    if day.len()!=10 { return Err("Formato dia debe ser YYYY-MM-DD".into()); }
    let src = std::path::Path::new(&xls_path);
    if !src.exists() { return Err(format!("XLS no existe: {}", xls_path)); }
    // Gate de integridad: si la columna SKU es numerica, los ceros a la
    // izquierda se perdieron al re-guardar el XLS en Excel. Bloquear: importar
    // crearia SKUs falsos que colisionan con otros de distinta linea.
    reject_numeric_sku(&xls_path)?;
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
    // Parsear e insertar en la BD (insert_ventas reemplaza el rango del dia)
    match g360_db_ventas::processor::parser::parse_export_csv(&dest_csv) {
        Ok((ventas, _stats)) if !ventas.is_empty() => {
            let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
            let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas, true).await.map_err(|e| e.to_string())?;
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
pub async fn import_manual_month(month: String, xls_path: String) -> Result<String, String> {
    // month: YYYY-MM, xls_path: XLS mensual completo descargado manualmente del intranet
    if month.len()!=7 || !month.contains('-') { return Err("Formato mes debe ser YYYY-MM".into()); }
    let src = std::path::Path::new(&xls_path);
    if !src.exists() { return Err(format!("XLS no existe: {}", xls_path)); }
    // Gate de integridad: mismo chequeo que import_manual_day.
    reject_numeric_sku(&xls_path)?;
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
    // Parsear + insertar (insert_ventas reemplaza el mes completo)
    match g360_db_ventas::processor::parser::parse_export_csv(&dest_csv) {
        Ok((ventas, _stats)) if !ventas.is_empty() => {
            let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
            let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas, true).await.map_err(|e| e.to_string())?;
            // Limpiar failed json del mes
            let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", month)));
            Ok(format!("Mes {} importado desde XLS manual: {} filas insertadas", month, n))
        }
        Ok(_) => Err("Parse devolvio 0 filas — verifica que el XLS sea del reporte Estadistica11".into()),
        Err(e) => Err(format!("Parse fallo: {}", e)),
    }
}

#[tauri::command]
pub async fn preview_import(xls_path: String) -> Result<PreviewResult, String> {
    let src = std::path::Path::new(&xls_path);
    if !src.exists() { return Err(format!("XLS no existe: {}", xls_path)); }
    // Convertir a CSV temporal via calamine (no toca raw/)
    let tmp_csv = std::env::temp_dir().join(format!("preview_{}.csv", chrono::Utc::now().timestamp_millis()));
    g360_db_ventas::processor::xls::xls_to_csv(src, &tmp_csv).map_err(|e| e.to_string())?;
    let (ventas, _stats) = g360_db_ventas::processor::parser::parse_export_csv(&tmp_csv).map_err(|e| e.to_string())?;
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
    let mut warnings: Vec<String> = Vec::new();
    if ventas.is_empty() {
        warnings.push("Parse devolvio 0 filas — verifica que sea Estadistica11".to_string());
    }
    // Aviso temprano: si la columna SKU es numerica, el import se bloqueara.
    if let Ok(insp) = g360_db_ventas::processor::xls::inspect_sku_column(src) {
        if insp.has_numeric_sku() {
            warnings.push(format!(
                "⛔ Columna SKU guardada como NÚMERO ({} filas). El cero inicial se pudo perder \
                 (011019 → 11019) y la importación será bloqueada. Re-exporta el XLS del intranet \
                 sin re-guardarlo en Excel. {}",
                insp.numeric_cells,
                insp.warning_message()
            ));
        }
    }
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
pub async fn reparse_raw() -> Result<String, String> {
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
                    match g360_db_ventas::db::writer::insert_ventas(&pool, &ventas, false).await {
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
        let _ = g360_db_ventas::db::writer::refresh_stats_cache(&pool).await;
        let mut st = shared_err.lock().unwrap();
        st.set_phase(CapturePhase::Done, &format!("Re-parse completado: {} filas", total));
        st.update_progress(1.0, &format!("Re-parse completado: {} filas", total));
    });

    Ok("Reprocesando raw...".to_string())
}

#[tauri::command]
pub async fn delete_month(month: String) -> Result<String, String> {
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
pub async fn delete_months(months: Vec<String>) -> Result<String, String> {
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

#[tauri::command]
pub async fn preview_csv() -> Result<Vec<Vec<String>>, String> {
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

