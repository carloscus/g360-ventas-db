use crate::config::{get_supabase_key, get_supabase_service_key, get_supabase_url, SUPABASE_TABLE};
use crate::models::Venta;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

/// Callback opcional para reportar progreso durante el upload.
/// (batch_actual, total_batches, porcentaje, mensaje)
pub type ProgressCb = Option<Arc<dyn Fn(usize, usize, f32, &str) + Send + Sync>>;

/// Sube un batch de ventas a Supabase (sin on_conflict para evitar error 500 con multi-línea).
/// Reintenta hasta 3 veces con backoff exponencial ante errores transitorios (5xx, timeout).
/// Deduplica por (folio_unico, id_articulo) para evitar duplicados en Supabase.
pub async fn upload_to_supabase(ventas: &[Venta], progress_cb: &ProgressCb) -> Result<usize> {
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Supabase credentials not configured"));
    }
    // Deduplicar por (folio_unico, id_articulo): mantener la primera ocurrencia de cada línea
    let mut seen: std::collections::HashMap<(String, String), ()> = std::collections::HashMap::new();
    let deduped: Vec<Venta> = ventas.iter()
        .filter(|v| seen.insert((v.folio_unico.clone(), v.id_articulo.clone()), ()).is_none())
        .cloned()
        .collect();
    if deduped.len() < ventas.len() {
        info!("Upload dedup: {} duplicados descartados en batch de {}", ventas.len() - deduped.len(), ventas.len());
    }
    // NO on_conflict: Supabase no permite múltiples filas con misma key en upsert batch.
    // La deduplicación se maneja a nivel de aplicación (reparse + dedup_ventas).
    let endpoint = format!(
        "{}/rest/v1/{}",
        url, SUPABASE_TABLE
    );
    let body = serde_json::to_string(&deduped)?;
    let client = reqwest::Client::new();

    let max_attempts = 3;
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=max_attempts {
        let resp = match client
            .post(&endpoint)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .header("Prefer", "resolution=merge-duplicates")
            .header("Accept", "application/json")
            .body(body.clone())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let e2 = e.to_string();
                last_err = Some(e.into());
                if attempt < max_attempts {
                    tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                    continue;
                }
                return Err(anyhow::anyhow!("Supabase request failed: {e2}"));
            }
        };
        let st = resp.status();
        let txt = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                let e2 = e.to_string();
                last_err = Some(e.into());
                if attempt < max_attempts {
                    tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                    continue;
                }
                return Err(anyhow::anyhow!("Supabase read response failed: {e2}"));
            }
        };
        info!("Supabase: {} ({} chars)", st, txt.len());
        if st.is_success() {
            if let Some(cb) = progress_cb {
                cb(0, 0, 0.0, ""); // no-op batch done
            }
            return Ok(deduped.len());
        }
        // Errores transitorios (5xx, rate-limit) -> reintentar; 4xx -> error inmediato
        if st.is_server_error() || st == reqwest::StatusCode::TOO_MANY_REQUESTS {
            last_err = Some(anyhow::anyhow!(
                "Supabase {}: {}",
                st,
                &txt.chars().take(100).collect::<String>()
            ));
            if attempt < max_attempts {
                tokio::time::sleep(Duration::from_millis(1000 * attempt as u64)).await;
                continue;
            }
        }
        return Err(anyhow::anyhow!("Supabase {}: {}", st, txt));
    }
    Err(last_err
        .unwrap_or_else(|| anyhow::anyhow!("Supabase upload failed after {} attempts", max_attempts)))
}

/// Sube todas las ventas desde SQLite a Supabase en batches de 500.
/// Usa last_supabase_sync para sync incremental: solo sube registros capturados despues
/// de la ultima sincronizacion exitosa.
/// Retorna tupla: (rows_subidos, rows_limpiados_por_retencion).
pub async fn upload_all(
    pool: &sqlx::SqlitePool,
    retention_days: u32,
    last_sync: Option<&str>,
    progress_cb: &ProgressCb,
) -> Result<(usize, usize)> {
    let bs = 500;

    // VALIDACIÓN: verificar duplicados locales antes de subir
    let dup_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT folio_unico, id_articulo, COUNT(*) as cnt
            FROM ventas
            GROUP BY folio_unico, id_articulo
            HAVING COUNT(*) > 1
        )"
    )
    .fetch_one(pool)
    .await
    .context("Duplicate check failed")?;

    if dup_count > 0 {
        return Err(anyhow::anyhow!(
            "Hay {} pares (folio,SKU) duplicados en BD local. Ejecuta 'Reprocesar raw' primero para deduplicar.",
            dup_count
        ));
    }

    // Full-sync (sin last_sync): purgar ventana completa en Supabase ANTES de re-subir,
    // si no habría duplicados (no hay constraint única desde 20250901000000_fix_folio_unique.sql).
    if last_sync.is_none() || last_sync == Some("") {
        if let Some(cb) = progress_cb {
            cb(0, 0, 0.02, "Full sync: purgando ventana en Supabase...");
        }
        match purge_supabase_window(retention_days).await {
            Ok(purged) => info!("Full sync: {} filas purgadas en ventana de retencion", purged),
            Err(e) => return Err(anyhow::anyhow!("Full sync: fallo purge de ventana: {}", e)),
        }
    }

    // Filtro incremental: solo registros desde last_supabase_sync
    let sync_where = match last_sync {
        Some(s) if !s.is_empty() => format!("WHERE capturado_en > '{}'", s),
        _ => String::new(),
    };

    // Calcular cutoff de retencion (mes alineado: mes_ref es 'YYYY-MM')
    let retention_cutoff_date = if retention_days > 0 {
        let d = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .format("%Y-%m-%d")
            .to_string();
        Some(d[..7].to_string())
    } else {
        None
    };

    // Combinar filtros: sync + retención
    let base_where = match (sync_where.as_str(), &retention_cutoff_date) {
        ("", None) => String::new(), // Sin filtros
        ("", Some(cutoff)) => format!("WHERE mes_ref >= '{}'", cutoff), // Solo retención
        (where_clause, None) => where_clause.to_string(), // Solo sync
        (where_clause, Some(cutoff)) => format!("{} AND mes_ref >= '{}'", where_clause, cutoff), // Ambos
    };

    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM ventas {}", base_where
    ))
    .fetch_one(pool)
    .await
    .context("Count query failed")?;

    if count == 0 {
        return Ok((0, 0));
    }

    let total_batches = ((count as f64) / (bs as f64)).ceil() as usize;
    let mut up = 0usize;

    let select_sql = format!(
        "SELECT id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,\
         id_tipo,nom_tipo,id_familia,nom_familia,\
         id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,\
         moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,\
         fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,\
         departamento,provincia,distrito,id_vendedor,\
         nom_vendedor,id_pedido,file_source,mes_ref,\
         tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico \
         FROM ventas {} LIMIT ? OFFSET ?", base_where
    );

    let start = Instant::now();
    for off in (0..count as usize).step_by(bs) {
        let batch_num = (off / bs) + 1;
        let pct_upload = (off as f64 / count as f64) * 0.9;
        if let Some(cb) = progress_cb {
            cb(batch_num, total_batches, pct_upload as f32, &format!("Batch {}/{}", batch_num, total_batches));
        }

        let rows: Vec<Venta> = sqlx::query(&select_sql)
            .bind(bs as i64)
            .bind(off as i64)
            .map(|row: sqlx::sqlite::SqliteRow| {
                use sqlx::Row;
                Venta {
                    id_articulo: row.get(0),
                    original_sku: row.get(1),
                    nom_articulo: row.get(2),
                    id_linea: row.get(3),
                    nom_linea: row.get(4),
                    id_grupo: row.get(5),
                    nom_grupo: row.get(6),
                    id_tipo: row.get(7),
                    nom_tipo: row.get(8),
                    id_familia: row.get(9),
                    nom_familia: row.get(10),
                    id_cliente: row.get(11),
                    doc_cliente: row.get(12),
                    nom_cliente: row.get(13),
                    tpo_doc: row.get(14),
                    serie_doc: row.get(15),
                    nro_doc: row.get(16),
                    referencia: row.get(17),
                    moneda: row.get(18),
                    cantidad: row.get(19),
                    cantidad_fae: row.try_get(20).unwrap_or_default(),
                    soles: row.get(21),
                    dolares: row.get(22),
                    precio_unitario: row.get(23),
                    anho: row.get(24),
                    mes: row.get(25),
                    fecha_orig: row.get(26),
                    fecha_ref: row.try_get(27).unwrap_or(None),
                    fecha_venc: row.try_get(28).unwrap_or(None),
                    cod_sucursal: row.get(29),
                    nom_sucursal: row.get(30),
                    departamento: row.get(31),
                    provincia: row.get(32),
                    distrito: row.get(33),
                    id_vendedor: row.get(34),
                    nom_vendedor: row.get(35),
                    id_pedido: row.get(36),
                    file_source: row.get(37),
                    mes_ref: row.get(38),
                    tipo_operacion: row.try_get(39).unwrap_or_default(),
                    factura_ref_serie: row.try_get(40).unwrap_or_default(),
                    factura_ref_nro: row.try_get(41).unwrap_or_default(),
                    folio_unico: row.try_get(42).unwrap_or_default(),
                }
            })
            .fetch_all(pool)
            .await
            .context("Fetch batch failed")?;

        match upload_to_supabase(&rows, progress_cb).await {
            Ok(n) => up += n,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Error en batch {}/{} ({:.0}%): {}",
                    batch_num,
                    total_batches,
                    pct_upload * 100.0,
                    e
                ));
            }
        }
    }

    // Retencion: eliminar registros fuera de ventana
    let cleaned = cleanup_supabase_retention(retention_days, progress_cb).await.unwrap_or(0);

    info!(
        "Upload done: {} rows in {:.1}s, {} retention-cleaned",
        up,
        start.elapsed().as_secs_f64(),
        cleaned
    );
    Ok((up, cleaned))
}

/// Elimina de Supabase los registros DENTRO de la ventana de retención (full sync).
/// Borra por rangos de mes_ref (menos filas por request que por id, evita timeouts).
/// Solo se llama desde full-sync (last_sync=None) antes de re-subir la ventana.
pub async fn purge_supabase_window(retention_days: u32) -> Result<usize> {
    if retention_days == 0 {
        return Ok(0);
    }
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Ok(0);
    }
    let cutoff_date = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
        .format("%Y-%m-%d")
        .to_string();
    // cutoff sobre mes_ref: '2023-03-05' -> meses >= '2023-03'
    let cutoff_mes = &cutoff_date[..7];

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);
    let mut purged = 0usize;

    // DELETE en loop: cada request borra hasta ~50K filas (límite PostgREST),
    // repetir hasta que no quede nada en la ventana.
    loop {
        let resp = client
            .delete(&base_url)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Prefer", "return=representation")
            .query(&[("mes_ref", format!("gte.{}", cutoff_mes).as_str()), ("select", "id")])
            .send()
            .await
            .context("Supabase purge request failed")?;

        let st = resp.status();
        if !st.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Supabase purge {}: {}", st, txt.chars().take(150).collect::<String>()));
        }
        let rows: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));
        let n = rows.as_array().map(|a| a.len()).unwrap_or(0);
        purged += n;
        info!("Purge batch: {} filas (acumulado {})", n, purged);
        if n == 0 {
            break;
        }
    }

    Ok(purged)
}

/// Elimina de Supabase los registros fuera de la ventana de retencion.
/// Usa DELETE por ID en batches de 500 para mayor fiabilidad (el DELETE con filtro de rango
/// de Supabase puede borrar todos los registros de una sola vez sin control de cantidad).
pub async fn cleanup_supabase_retention(
    retention_days: u32,
    progress_cb: &ProgressCb,
) -> Result<usize> {
    if retention_days == 0 {
        return Ok(0); // retencion ilimitada
    }
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Ok(0);
    }
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
        .format("%Y-%m-%d")
        .to_string();
    // mes alineado: borrar por mes_ref evita borrar dias sueltos del mes-corte
    let cutoff_mes = cutoff[..7].to_string();
    let client = reqwest::Client::new();
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);

    // Contar cuantos IDs hay fuera de ventana
    let count_endpoint = format!(
        "{}?mes_ref=lt.{}&select=id&limit=1",
        base_url, cutoff_mes
    );
    let count_resp = client
        .get(&count_endpoint)
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Prefer", "count=exact")
        .header("Range", "0-0")
        .send()
        .await
        .context("Supabase count request failed")?;
    let content_range = count_resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let total_old: usize = content_range
        .split('/')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if total_old == 0 {
        info!("Supabase retention: nada fuera de ventana (< {})", cutoff);
        return Ok(0);
    }

    if let Some(cb) = progress_cb {
        cb(0, 0, 0.95, &format!("Limpiando {} registros antiguos...", total_old));
    }

    info!(
        "Supabase retention: {} registros fuera de ventana (< {}), eliminando...",
        total_old, cutoff
    );

    // DELETE directo por mes_ref (loop hasta vaciar; PostgREST borra todo el rango por request)
    let mut deleted = 0usize;

    loop {
        let resp = client
            .delete(&base_url)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Prefer", "return=representation")
            .query(&[("mes_ref", format!("lt.{}", cutoff_mes).as_str()), ("select", "id")])
            .send()
            .await
            .context("Supabase retention delete failed")?;

        if !resp.status().is_success() {
            let txt = resp.text().await.unwrap_or_default();
            info!("Supabase retention delete fallo: {}", txt.chars().take(120).collect::<String>());
            break;
        }
        let rows: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));
        let n = rows.as_array().map(|a| a.len()).unwrap_or(0);
        deleted += n;

        if let Some(cb) = progress_cb {
            let frac = if total_old > 0 { (deleted as f64 / total_old as f64).min(1.0) } else { 1.0 };
            cb(0, 0, (0.95 + frac * 0.05) as f32, &format!(
                "Retencion: {}/{} limpiados",
                deleted, total_old
            ));
        }

        if n == 0 {
            break;
        }
    }

    info!(
        "Supabase retention: {} registros eliminados de {}",
        deleted, total_old
    );
    Ok(deleted)
}

pub async fn test_supabase_connection(url: &str, key: &str) -> Result<String> {
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Ingrese credenciales validas"));
    }
    let client = reqwest::Client::new();
    let endpoint = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);
    let resp = client
        .get(&endpoint)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Prefer", "return=minimal")
        .header("Range", "0-0")
        .send()
        .await
        .context("No se pudo conectar a Supabase")?;
    let st = resp.status();
    if st.is_success() {
        Ok(format!(
            "OK - tabla '{}' accesible ({})",
            SUPABASE_TABLE, st
        ))
    } else {
        let txt = resp.text().await.unwrap_or_default();
        Ok(format!(
            "ERROR {}: {}",
            st,
            txt.chars().take(200).collect::<String>()
        ))
    }
}

// ─── CAPAS DE PROTECCIÓN PARA UPLOAD ────────────────────────────────────────

/// Estructura de resultados para validación pre-upload
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub total_rows: usize,
    pub valid_rows: usize,
    pub invalid_rows: Vec<String>,
    pub warnings: Vec<String>,
}

/// Valida la calidad de los datos ANTES de subir
pub async fn validate_upload_data(
    pool: &sqlx::SqlitePool,
    retention_days: u32,
    last_sync: Option<&str>,
) -> Result<ValidationReport> {
    // Calcular cutoff de retención
    let cutoff = if retention_days > 0 {
        Some((chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .format("%Y-%m-%d")
            .to_string())
    } else {
        None
    };
    
    // Filtro incremental
    let sync_filter = match last_sync {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    };
    
    // Contar filas que se subirían
    let count_query = if let Some(ref c) = cutoff {
        if let Some(ref s) = sync_filter {
            format!("SELECT COUNT(*) FROM ventas WHERE mes_ref >= '{}' AND capturado_en > '{}'", c, s)
        } else {
            format!("SELECT COUNT(*) FROM ventas WHERE mes_ref >= '{}'", c)
        }
    } else if let Some(ref s) = sync_filter {
        format!("SELECT COUNT(*) FROM ventas WHERE capturado_en > '{}'", s)
    } else {
        "SELECT COUNT(*) FROM ventas".to_string()
    };
    
    let total: i64 = sqlx::query_scalar(&count_query)
        .fetch_one(pool)
        .await?;
    
    // Validar que no haya nulos en campos críticos
    let null_checks = vec![
        ("sin_folio", "SELECT COUNT(*) FROM ventas WHERE folio_unico IS NULL OR folio_unico = ''"),
        ("sin_sku", "SELECT COUNT(*) FROM ventas WHERE id_articulo IS NULL OR id_articulo = ''"),
        ("cant_neg", "SELECT COUNT(*) FROM ventas WHERE cantidad < 0"),
    ];
    
    let mut invalid_rows = Vec::new();
    for (name, query) in null_checks {
        let cnt: i64 = sqlx::query_scalar(query).fetch_one(pool).await.unwrap_or(0);
        if cnt > 0 {
            invalid_rows.push(format!("{}: {} registros", name, cnt));
        }
    }
    
    Ok(ValidationReport {
        total_rows: total as usize,
        valid_rows: total as usize,
        invalid_rows,
        warnings: Vec::new(),
    })
}

/// Verifica post-upload: compara filas enviadas vs filas recibidas en Supabase.
/// `since_utc` filtra por capturado_en >= timestamp de inicio del upload (las filas
/// de esta corrida); sin él compararía contra el total de la tabla (siempre dispara
/// falso negativo en sync incremental).
pub async fn verify_upload_result(
    supabase_url: &str,
    supabase_key: &str,
    expected_count: usize,
    since_utc: Option<&str>,
) -> Result<VerificationResult> {
    use crate::config::get_supabase_service_key;

    let key = if supabase_key.is_empty() {
        get_supabase_service_key()
    } else {
        supabase_key.to_string()
    };

    let client = reqwest::Client::new();
    let mut endpoint = format!("{}/rest/v1/{}", supabase_url, SUPABASE_TABLE);
    if let Some(since) = since_utc {
        // capturado_en es timestamptz con default now() al insertar: las filas de
        // esta corrida tienen capturado_en >= inicio del upload
        endpoint = format!("{}?capturado_en=gte.{}", endpoint, since);
    }

    // Get current count from Supabase
    let resp = client
        .get(&endpoint)
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Prefer", "count=exact")
        .header("Range", "0-0")
        .send()
        .await
        .context("Failed to verify upload")?;

    let content_range = resp.headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("0/0");

    let actual_count: usize = content_range
        .split('/')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let matched = actual_count == expected_count;

    Ok(VerificationResult {
        expected_count,
        actual_count,
        matched,
        discrepancy: if matched { 0 } else { expected_count.abs_diff(actual_count) },
    })
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub expected_count: usize,
    pub actual_count: usize,
    pub matched: bool,
    pub discrepancy: usize,
}

/// Dry-run: calcula cuántas filas se subirían sin hacer upload real
pub async fn dry_run_upload(
    pool: &sqlx::SqlitePool,
    retention_days: u32,
    last_sync: Option<&str>,
) -> Result<DryRunResult> {
    let bs = 500;
    
    // Calcular filtro de retención
    let retention_cutoff = if retention_days > 0 {
        Some((chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .format("%Y-%m-%d")
            .to_string())
    } else {
        None
    };
    
    // Construir WHERE clause
    let sync_where = match last_sync {
        Some(s) if !s.is_empty() => Some(format!("WHERE capturado_en > '{}'", s)),
        _ => None,
    };
    
    let base_where = match (&sync_where, &retention_cutoff) {
        (None, None) => String::new(),
        (None, Some(cutoff)) => format!("WHERE mes_ref >= '{}'", cutoff),
        (Some(w), None) => w.clone(),
        (Some(w), Some(cutoff)) => format!("{} AND mes_ref >= '{}'", w, cutoff),
    };
    
    // Contar filas a subir
    let count_query = format!(
        "SELECT COUNT(*) FROM ventas {}",
        if base_where.is_empty() { "".to_string() } else { format!(" {}", base_where) }
    );
    
    let count: i64 = sqlx::query_scalar(&count_query)
        .fetch_one(pool)
        .await
        .context("Count query failed")?;
    
    let total_batches = ((count as f64) / (bs as f64)).ceil() as usize;
    
    // Calcular estadísticas del lote
    let stats_query = format!(
        "SELECT SUM(cantidad), SUM(soles), SUM(COALESCE(dolares, 0)), COUNT(DISTINCT folio_unico), MIN(mes_ref), MAX(mes_ref) FROM ventas {}",
        if base_where.is_empty() { "".to_string() } else { format!(" {}", base_where) }
    );
    
    let stats: Option<(f64, f64, f64, i64, String, String)> = sqlx::query_as(&stats_query)
        .fetch_optional(pool)
        .await?;
    
    Ok(DryRunResult {
        rows_to_upload: count as usize,
        total_batches,
        total_cantidad: stats.as_ref().map(|s| s.0).unwrap_or(0.0),
        total_soles: stats.as_ref().map(|s| s.1).unwrap_or(0.0),
        total_dolares: stats.as_ref().map(|s| s.2).unwrap_or(0.0),
        unique_invoices: stats.as_ref().map(|s| s.3).unwrap_or(0),
        date_range_start: stats.as_ref().map(|s| s.4.clone()),
        date_range_end: stats.as_ref().map(|s| s.5.clone()),
    })
}

#[derive(Debug, Clone)]
pub struct DryRunResult {
    pub rows_to_upload: usize,
    pub total_batches: usize,
    pub total_cantidad: f64,
    pub total_soles: f64,
    pub total_dolares: f64,
    pub unique_invoices: i64,
    pub date_range_start: Option<String>,
    pub date_range_end: Option<String>,
}
