use crate::config::{get_supabase_service_key, get_supabase_url, SUPABASE_TABLE};
use crate::models::Venta;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

use super::*;

/// Callback opcional para reportar progreso durante el upload.
/// (batch_actual, total_batches, porcentaje, mensaje)
pub type ProgressCb = Option<Arc<dyn Fn(usize, usize, f32, &str) + Send + Sync>>;

/// Sube un batch de ventas a Supabase.
/// Reintenta hasta 3 intentos con backoff exponencial ante errores transitorios (5xx, timeout).
/// NO deduplica: `(folio_unico, id_articulo)` NO es unico en el ERP — una factura puede
/// tener varias lineas del mismo SKU (gratuita + pagada, precios distintos, repeticion
/// por guia de remision). Filtrar por esa clave borra datos reales (verificado 2026-09).
pub async fn upload_to_supabase(ventas: &[Venta], progress_cb: &ProgressCb) -> Result<usize> {
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Supabase credentials not configured"));
    }
    // NO on_conflict: Supabase no permite múltiples filas con misma key en upsert batch.
    let endpoint = format!(
        "{}/rest/v1/{}",
        url, SUPABASE_TABLE
    );
    // Supabase no tiene estas columnas (raw-first: viven solo en el SQLite local).
    // PostgREST rechaza claves desconocidas con PGRST204, asi que se excluyen del
    // payload de subida (decisión: no migrar estas columnas a Supabase).
    const SUPABASE_EXCLUDED_COLS: [&str; 8] = [
        "ord_compra",
        "id_ubigeo",
        "canal_distribucion",
        "nom_condicion_pago",
        "estado_linea",
        "division",
        "fec_cargo",
        "id_guia",
    ];
    let payload: Vec<serde_json::Value> = ventas
        .iter()
        .map(|v| {
            let mut m = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
            if let Some(obj) = m.as_object_mut() {
                for c in SUPABASE_EXCLUDED_COLS {
                    obj.remove(c);
                }
            }
            m
        })
        .collect();
    let body = serde_json::to_string(&payload)?;
    // Timeout: sin esto, un PostgREST colgado (statement timeout en tabla con churn
    // masivo) deja el upload eternamente esperando.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .build()?;

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
            return Ok(ventas.len());
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
    // La retención se decide por mes de venta (`mes_ref`), nunca por
    // `capturado_en`. Este último solo indica que el mes fue recapturado.
    let retention_cutoff_month = if retention_days > 0 {
        Some(
            (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
                .format("%Y-%m")
                .to_string(),
        )
    } else {
        None
    };

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

    // RAW-FIRST: filtro OBLIGATORIO de líneas (allowlist + regla NC/ND) al subir.
    // La BD local guarda todo el export del ERP; Supabase solo recibe lo vital.
    let upload_where = crate::config::with_line_filter(&base_where);

    let invalid_dates: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM ventas {} AND (
            fecha_orig = '0001-01-01'
            OR date(fecha_orig) IS NULL
            OR length(fecha_orig) != 10
            OR substr(fecha_orig, 5, 1) != '-'
            OR substr(fecha_orig, 8, 1) != '-'
            OR substr(fecha_orig, 1, 7) != substr(mes_ref, 1, 7)
        )",
        upload_where
    ))
    .fetch_one(pool)
    .await
    .context("Date validation query failed")?;
    if invalid_dates > 0 {
        return Err(anyhow::anyhow!(
            "Se encontraron {} filas elegibles con fecha inválida o fuera de su mes_ref. Corrige/reprocesa el archivo local antes de subir.",
            invalid_dates
        ));
    }

    // NOTA: NO se valida unicidad por (folio_unico, id_articulo). Esa clave NO es
    // unica en el ERP: una factura puede tener varias lineas del mismo SKU (linea
    // gratuita + pagada, precios distintos, o repeticion por guia de remision).
    // Se suben TODAS las filas elegibles; Supabase no tiene constraint de unicidad.

    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM ventas {}", upload_where
    ))
    .fetch_one(pool)
    .await
    .context("Count query failed")?;

    // Visibilidad raw-first: cuántas filas locales quedan fuera por el filtro
    let total_local: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    info!(
        "Filtro de líneas: {}/{} filas locales elegibles para Supabase ({} excluidas por allowlist/NC-ND)",
        count, total_local, total_local - count
    );

    if count == 0 {
        return Ok((0, 0));
    }

    // Todas las purgas se ejecutan DESPUÉS de validar el lote local (fechas +
    // duplicados): un error de calidad nunca debe dejar Supabase parcialmente vacío.
    if last_sync.is_none() || last_sync == Some("") {
        if let Some(cb) = progress_cb {
            cb(0, 0, 0.02, "Full sync: purgando ventana en Supabase...");
        }
        match purge_supabase_window(retention_days).await {
            Ok(purged) => info!("Full sync: {} filas purgadas en ventana de retencion", purged),
            Err(e) => return Err(anyhow::anyhow!("Full sync: fallo purge de ventana: {}", e)),
        }
    } else if let Some(cb) = progress_cb {
        cb(0, 0, 0.01, "Verificando meses re-capturados...");
    }

    if !sync_where.is_empty() {
        match resync_stale_months(pool, last_sync, retention_cutoff_month.as_deref()).await {
            Ok(stale) if !stale.is_empty() => {
                info!("Month-aware sync: {} meses re-capturados detectados: {:?}", stale.len(), stale);
                for (mes, purged) in stale {
                    info!("Month-aware sync: mes {} purgado ({} filas)", mes, purged);
                }
            }
            Ok(_) => {}
            Err(e) => return Err(anyhow::anyhow!("Month-aware sync: fallo purga de meses: {}", e)),
        }
    }

    let total_batches = ((count as f64) / (bs as f64)).ceil() as usize;
    let mut up = 0usize;

    // KEYSET PAGINATION: WHERE id > last ORDER BY id LIMIT n (costo constante por
    // batch, usa PK). El OFFSET previo era O(n^2): en el batch ~950 tenia que
    // recorrer 475K filas antes de leer las suyas -> la subida se degradaba de
    // ~2 batches/s a minutos por batch (bug real 2026-09-09, RAM 4.5 GB incluida).
    let select_sql = format!(
        "SELECT {} FROM ventas {} AND id > ? ORDER BY id LIMIT ?",
        VENTA_SELECT_COLS, upload_where
    );

    let start = Instant::now();
    let mut last_id: i64 = 0;
    let mut batch_num = 0usize;
    loop {
        batch_num += 1;
        let pct_upload = (up.min(count as usize) as f64 / count as f64) * 0.9;
        if let Some(cb) = progress_cb {
            cb(batch_num, total_batches, pct_upload as f32, &format!("Batch {}/{}", batch_num, total_batches));
        }

        let rows: Vec<(i64, Venta)> = sqlx::query(&select_sql)
            .bind(last_id)
            .bind(bs as i64)
            .map(map_venta_row)
            .fetch_all(pool)
            .await
            .context("Fetch batch failed")?;

        if rows.is_empty() {
            break; // ya no hay filas elegibles: upload completo
        }
        last_id = rows.last().map(|(id, _)| *id).unwrap_or(last_id);

        match upload_to_supabase(&rows.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), progress_cb).await {
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
        if up >= count as usize {
            break;
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

/// Sincroniza UN mes pero SOLO los días `>= fecha_floor`: purga esos días en
/// Supabase y re-sube las filas locales elegibles de esos días. Devuelve (purgadas, subidas).
pub async fn sync_month_days(
    pool: &sqlx::SqlitePool,
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    mes: &str,
    fecha_floor: &str,
) -> Result<(usize, usize)> {
    let purged = delete_days_in_month_batched(client, base_url, key, mes, fecha_floor).await?;

    let where_clause = crate::config::with_line_filter(&format!(
        "WHERE mes_ref = '{}' AND fecha_orig >= '{}'",
        mes.replace('\'', "''"),
        fecha_floor.replace('\'', "''")
    ));
    let select_sql = format!(
        "SELECT {} FROM ventas {} AND id > ? ORDER BY id LIMIT ?",
        VENTA_SELECT_COLS, where_clause
    );
    let no_cb: ProgressCb = None;
    let mut last_id = 0i64;
    let mut uploaded = 0usize;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 100_000 {
            return Err(anyhow::anyhow!("sync_month_days {mes}: demasiados batches"));
        }
        let rows: Vec<(i64, Venta)> = sqlx::query(&select_sql)
            .bind(last_id)
            .bind(500i64)
            .map(map_venta_row)
            .fetch_all(pool)
            .await
            .context("sync_month_days fetch failed")?;
        if rows.is_empty() {
            break;
        }
        let len = rows.len();
        last_id = rows.last().map(|(id, _)| *id).unwrap_or(last_id);
        let ventas: Vec<Venta> = rows.into_iter().map(|(_, v)| v).collect();
        uploaded += upload_to_supabase(&ventas, &no_cb).await?;
        if len < 500 {
            break;
        }
    }
    Ok((purged, uploaded))
}

/// Sync INCREMENTAL por DÍA (día a día): solo purga + re-sube los días en
/// `[desde, hasta]`, NO el mes completo. Evita duplicados (purga antes de subir) y
/// reduce el bloat frente a reconciliar meses enteros cuando solo cambió un día.
/// Tras sincronizar, marca los meses tocados como sincronizados (`sync_months`).
pub async fn sync_incremental(
    pool: &sqlx::SqlitePool,
    retention_days: u32,
    desde: &str,
    hasta: &str,
    progress_cb: &ProgressCb,
) -> Result<ReconcileResult> {
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Supabase no configurado"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);

    // Limpieza fuera de ventana (best-effort), para que la ventana quede acotada.
    if let Some(cb) = progress_cb {
        cb(0, 0, 0.02, "Limpiando meses fuera de la ventana...");
    }
    let _ = cleanup_supabase_retention(retention_days, progress_cb).await;

    // Clamp de `desde` al inicio de la ventana de retención (mes alineado).
    let desde_eff = if retention_days > 0 {
        let cutoff_mes = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .format("%Y-%m")
            .to_string();
        let floor = format!("{}-01", cutoff_mes);
        if desde < floor.as_str() {
            floor
        } else {
            desde.to_string()
        }
    } else {
        desde.to_string()
    };

    let meses = month_span(&desde_eff, hasta)?;
    let mut synced = 0usize;
    let mut failed = 0usize;
    let mut uploaded = 0usize;
    let mut purged = 0usize;
    let mut consecutive = 0usize;
    let mut stopped_early = false;

    for (i, mes) in meses.iter().enumerate() {
        if let Some(cb) = progress_cb {
            let pct = 0.05 + 0.9 * (i as f64 / meses.len().max(1) as f64);
            cb(
                0,
                meses.len(),
                pct as f32,
                &format!("Sync incremental {}/{} ({})", i + 1, meses.len(), mes),
            );
        }
        // Día piso: en el mes de `desde` arranca en `desde`; en los siguientes, día 1.
        let floor = if mes.as_str() == &desde_eff[..7] {
            desde_eff.clone()
        } else {
            format!("{}-01", mes)
        };
        match sync_month_days(pool, &client, &base_url, &key, mes, &floor).await {
            Ok((p, u)) => {
                purged += p;
                uploaded += u;
                synced += 1;
                consecutive = 0;
                // Marcar el mes como sincronizado (remote = local).
                if let Ok(Some((checksum, filas))) = local_month_checksum(pool, mes).await {
                    let _ = sqlx::query(
                        "INSERT INTO sync_months (mes_ref, local_checksum, remote_checksum, filas, estado, actualizado_en) \
                         VALUES (?, ?, ?, ?, 'synced', datetime('now')) \
                         ON CONFLICT(mes_ref) DO UPDATE SET local_checksum = excluded.local_checksum, \
                            remote_checksum = excluded.local_checksum, filas = excluded.filas, \
                            estado = 'synced', actualizado_en = excluded.actualizado_en",
                    )
                    .bind(mes)
                    .bind(&checksum)
                    .bind(&checksum)
                    .bind(filas)
                    .execute(pool)
                    .await;
                }
                info!(
                    "sync_incremental {}: purgadas {} + subidas {} (>= {})",
                    mes, p, u, floor
                );
            }
            Err(e) => {
                failed += 1;
                consecutive += 1;
                tracing::warn!("sync_incremental {} fallo: {}", mes, e);
                if consecutive >= RECONCILE_MAX_CONSECUTIVE_FAILURES {
                    stopped_early = true;
                    break;
                }
            }
        }
    }

    Ok(ReconcileResult {
        months_total: meses.len(),
        months_synced: synced,
        months_failed: failed,
        rows_uploaded: uploaded,
        rows_purged: purged,
        stopped_early,
    })
}

/// Determina si hay que subir a Supabase. Chequeo LOCAL-FIRST, sin I/O a Supabase:
/// si hay filas elegibles capturadas despues del ultimo sync -> `true`.
/// Nunca sincronizado -> `true`. Sin filas nuevas -> `false`.
/// Evita full-scans remotos (COUNT / MAX(capturado_en)) que consumen Disk IO Budget.
pub async fn check_supabase_needs_sync(
    pool: &sqlx::SqlitePool,
    retention_days: u32,
    last_sync: Option<&str>,
) -> Result<bool> {
    let last_sync = match last_sync {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(true),
    };
    let mut parts = vec![format!("capturado_en > '{}'", last_sync.replace('\'', "''"))];
    if retention_days > 0 {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .format("%Y-%m")
            .to_string();
        parts.push(format!("mes_ref >= '{}'", cutoff));
    }
    let base_where = format!("WHERE {}", parts.join(" AND "));
    let where_clause = crate::config::with_line_filter(&base_where);
    let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM ventas {}", where_clause))
        .fetch_one(pool)
        .await
        .context("check local new rows failed")?;
    if n > 0 {
        info!("check_supabase: {} filas locales nuevas desde {} -> sync", n, last_sync);
    }
    Ok(n > 0)
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
            .format("%Y-%m")
            .to_string())
    } else {
        None
    };

    // Filtro incremental por sync
    let sync_where = match last_sync {
        Some(s) if !s.is_empty() => Some(format!("WHERE capturado_en > '{}'", s)),
        _ => None,
    };

    // WHERE base: sync + retención (dry-run refleja exactamente lo que subiría upload_all)
    let base_where = match (&sync_where, &retention_cutoff) {
        (None, None) => String::new(),
        (None, Some(cutoff)) => format!("WHERE mes_ref >= '{}'", cutoff),
        (Some(w), None) => w.clone(),
        (Some(w), Some(cutoff)) => format!("{} AND mes_ref >= '{}'", w, cutoff),
    };
    let upload_where = crate::config::with_line_filter(&base_where);

    let invalid_dates: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM ventas {} AND (
            fecha_orig = '0001-01-01'
            OR date(fecha_orig) IS NULL
            OR length(fecha_orig) != 10
            OR substr(fecha_orig, 5, 1) != '-'
            OR substr(fecha_orig, 8, 1) != '-'
            OR substr(fecha_orig, 1, 7) != substr(mes_ref, 1, 7)
        )",
        upload_where
    ))
    .fetch_one(pool)
    .await
    .context("Date validation query failed")?;
    if invalid_dates > 0 {
        return Err(anyhow::anyhow!(
            "Dry-run bloqueado: {} filas elegibles tienen fechas inválidas o fuera de su mes_ref.",
            invalid_dates
        ));
    }

    // Contar filas a subir
    let count_query = format!(
        "SELECT COUNT(*) FROM ventas {}", upload_where
    );

    let count: i64 = sqlx::query_scalar(&count_query)
        .fetch_one(pool)
        .await
        .context("Count query failed")?;

    let total_batches = ((count as f64) / (bs as f64)).ceil() as usize;

    // Calcular estadísticas del lote
    let stats_query = format!(
        "SELECT SUM(cantidad), SUM(soles), SUM(COALESCE(dolares, 0)), COUNT(DISTINCT folio_unico), MIN(mes_ref), MAX(mes_ref) FROM ventas {}",
        upload_where
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
