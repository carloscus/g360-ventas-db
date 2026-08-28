use crate::config::{get_supabase_key, get_supabase_url, SUPABASE_TABLE};
use crate::models::Venta;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

/// Callback opcional para reportar progreso durante el upload.
/// (batch_actual, total_batches, porcentaje, mensaje)
pub type ProgressCb = Option<Arc<dyn Fn(usize, usize, f32, &str) + Send + Sync>>;

/// Sube un batch de ventas a Supabase con upsert por folio_unico.
/// Reintenta hasta 3 veces con backoff exponencial ante errores transitorios (5xx, timeout).
/// Deduplica por folio_unico dentro del batch antes de enviar.
pub async fn upload_to_supabase(ventas: &[Venta], progress_cb: &ProgressCb) -> Result<usize> {
    let url = get_supabase_url();
    let key = get_supabase_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Supabase credentials not configured"));
    }
    // Deduplicar: mantener la primera ocurrencia de cada folio_unico
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let deduped: Vec<&Venta> = ventas.iter().filter(|v| {
        seen.insert(v.folio_unico.as_str(), 0).is_none()
    }).collect();
    if deduped.len() < ventas.len() {
        info!("Upload dedup: {} duplicados descartados en batch de {}", ventas.len() - deduped.len(), ventas.len());
    }
    let endpoint = format!(
        "{}/rest/v1/{}?on_conflict=folio_unico",
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
    retention_years: u32,
    last_sync: Option<&str>,
    progress_cb: &ProgressCb,
) -> Result<(usize, usize)> {
    let bs = 500;

    // Filtro incremental: solo registros desde last_supabase_sync
    let base_where = match last_sync {
        Some(s) if !s.is_empty() => format!("WHERE capturado_en > '{}'", s),
        _ => String::new(),
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
    let cleaned = cleanup_supabase_retention(retention_years, progress_cb).await.unwrap_or(0);

    info!(
        "Upload done: {} rows in {:.1}s, {} retention-cleaned",
        up,
        start.elapsed().as_secs_f64(),
        cleaned
    );
    Ok((up, cleaned))
}

/// Elimina de Supabase los registros fuera de la ventana de retencion.
/// Usa DELETE por ID en batches de 500 para mayor fiabilidad (el DELETE con filtro de rango
/// de Supabase puede borrar todos los registros de una sola vez sin control de cantidad).
pub async fn cleanup_supabase_retention(
    retention_years: u32,
    progress_cb: &ProgressCb,
) -> Result<usize> {
    if retention_years == 0 {
        return Ok(0); // retencion ilimitada
    }
    let url = get_supabase_url();
    let key = get_supabase_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Ok(0);
    }
    let cutoff = (chrono::Utc::now() - chrono::Duration::days((retention_years as i64) * 365))
        .format("%Y-%m-%d")
        .to_string();
    let client = reqwest::Client::new();
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);

    // Contar cuantos IDs hay fuera de ventana
    let count_endpoint = format!(
        "{}?fecha_orig=lt.{}&select=id&limit=1",
        base_url, cutoff
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

    // Obtener IDs y borrar en batches de 500
    let mut deleted = 0usize;
    let bs = 500;
    let mut offset = 0usize;

    loop {
        let id_endpoint = format!(
            "{}?fecha_orig=lt.{}&select=id&limit={}&offset={}",
            base_url, cutoff, bs, offset
        );
        let resp = client
            .get(&id_endpoint)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Prefer", "return=minimal")
            .send()
            .await
            .context("Supabase fetch IDs failed")?;
        if !resp.status().is_success() {
            break;
        }
        let ids: Vec<String> = match resp.json().await {
            Ok(v) => v,
            Err(_) => break,
        };
        if ids.is_empty() {
            break;
        }
        offset += ids.len();
        deleted += ids.len();

        // Borrar por ID en batch
        let id_list: String = ids.iter().map(|i| i.as_str()).collect::<Vec<_>>().join(",");
        let delete_by_id = format!("{}?id=eq.({})", base_url, id_list);
        if let Ok(r) = client
            .delete(&delete_by_id)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Prefer", "return=minimal")
            .send()
            .await
        {
            if !r.status().is_success() {
                info!("Supabase retention batch delete failed at offset {}", offset - ids.len());
            }
        }

        if let Some(cb) = progress_cb {
            let pct = 0.95 + (offset as f64 / total_old as f64) * 0.05;
            cb(0, 0, pct as f32, &format!(
                "Retencion: {}/{} limpiados",
                deleted, total_old
            ));
        }

        if ids.len() < bs {
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
