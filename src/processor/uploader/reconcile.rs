use crate::config::{get_supabase_service_key, get_supabase_url, SUPABASE_TABLE};
use crate::models::Venta;
use anyhow::{Context, Result};
use std::time::Duration;
use tracing::info;

use super::*;

/// Elimina de Supabase los registros DENTRO de la ventana de retención (full sync).
/// Borra por rangos de mes_ref (menos filas por request que por id, evita timeouts).
/// Solo se llama desde full-sync (last_sync=None) antes de re-subir la ventana.
/// MONTH-AWARE SYNC: detecta meses cuya versión local es más reciente que la de
/// Supabase (por re-captura, reparse o re-atribución del ERP) y los PURGA de
/// Supabase para que el upload los re-inserte completos.
///
/// Mecanismo: para cada mes_ref local con capturado_en > last_sync, compara el
/// MAX(capturado_en) del mes en Supabase. Si local > supabase -> mes desactualizado
/// en Supabase -> DELETE por mes_ref (loop hasta vaciar). Si el mes NO existe en
/// Supabase es un mes nuevo (normal, no requiere purga).
///
/// Retorna los meses purgados con su conteo.
pub async fn resync_stale_months(
    pool: &sqlx::SqlitePool,
    last_sync: Option<&str>,
    retention_cutoff_month: Option<&str>,
) -> Result<Vec<(String, usize)>> {
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Ok(vec![]);
    }

    // Meses locales tocados desde el ultimo sync (o todos si full sync)
    let cutoff = last_sync.unwrap_or("2000-01-01");
    let meses: Vec<(String, String)> = if let Some(cutoff_month) = retention_cutoff_month {
        sqlx::query_as(
            "SELECT mes_ref, MAX(capturado_en) FROM ventas
             WHERE capturado_en > ? AND mes_ref >= ?
             GROUP BY mes_ref ORDER BY mes_ref",
        )
        .bind(cutoff)
        .bind(cutoff_month)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT mes_ref, MAX(capturado_en) FROM ventas
             WHERE capturado_en > ? GROUP BY mes_ref ORDER BY mes_ref",
        )
        .bind(cutoff)
        .fetch_all(pool)
        .await?
    };

    if meses.is_empty() {
        return Ok(vec![]);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?;
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);
    let mut purgados: Vec<(String, usize)> = Vec::new();

    for (mes_ref, max_local) in &meses {
        let mes_filter = format!("eq.{}", mes_ref);
        // MAX(capturado_en) del mes en Supabase.
        // SIN count=exact: ese count es full-scan por mes (ver pg_stat_statements).
        // Si el mes no existe, el array viene vacio -> supa_max None -> se purga
        // (delete_month_batched retorna 0 rapido al no hallar ids).
        let resp = client
            .get(&base_url)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Range", "0-0")
            .query(&[
                ("mes_ref", mes_filter.as_str()),
                ("order", "capturado_en.desc"),
                ("select", "capturado_en"),
            ])
            .send()
            .await
            .context("Month-aware: fallo consulta de mes en Supabase")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Month-aware: Supabase {} al consultar mes {}: {}",
                resp.status(), mes_ref,
                resp.text().await.unwrap_or_default().chars().take(120).collect::<String>()
            );
        }

        // Comparar: capturado_en max local vs max supabase (ambos ISO strings comparables)
        let supa_max: Option<String> = resp.json::<serde_json::Value>().await
            .ok()
            .and_then(|v| v.as_array().and_then(|a| a.first().cloned()))
            .and_then(|row| row.get("capturado_en").and_then(|c| c.as_str()).map(String::from));
        let local_is_newer = match supa_max {
            Some(s) => max_local.as_str() > s.as_str(),
            None => true,
        };
        if !local_is_newer {
            continue;
        }

        // Purgar el mes desactualizado (por lotes de ids, evita statement_timeout)
        let purged_mes = delete_month_batched(&client, &base_url, &key, mes_ref)
            .await
            .context("Month-aware: fallo DELETE de mes")?;
        if purged_mes > 0 {
            info!("Month-aware: mes {} desactualizado -> purgadas {} filas", mes_ref, purged_mes);
            purgados.push((mes_ref.clone(), purged_mes));
        }
    }

    Ok(purgados)
}

/// Resultado de una corrida de reconciliacion.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconcileResult {
    pub months_total: usize,
    pub months_synced: usize,
    pub months_failed: usize,
    pub rows_uploaded: usize,
    pub rows_purged: usize,
    pub stopped_early: bool,
}

/// Circuit breaker: corta la reconciliacion tras N meses fallidos consecutivos
/// (evita martillar Supabase cuando esta throttled/degradado).
pub const RECONCILE_MAX_CONSECUTIVE_FAILURES: usize = 3;

/// Sincroniza UN mes: purga sus filas en Supabase (por lotes de ids) y re-sube
/// las filas elegibles desde SQLite (keyset por id). Devuelve (purgadas, subidas).
pub async fn reconcile_month(
    pool: &sqlx::SqlitePool,
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    mes: &str,
) -> Result<(usize, usize)> {
    // 1) Purga el mes (borrado por lotes, evita statement_timeout)
    let purged = delete_month_batched(client, base_url, key, mes).await?;

    // 2) Sube las filas elegibles del mes desde SQLite
    let mes_esc = mes.replace('\'', "''");
    let where_clause =
        crate::config::with_line_filter(&format!("WHERE mes_ref = '{}'", mes_esc));
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
            return Err(anyhow::anyhow!("reconcile_month {mes}: demasiados batches"));
        }
        let rows: Vec<(i64, Venta)> = sqlx::query(&select_sql)
            .bind(last_id)
            .bind(500i64)
            .map(map_venta_row)
            .fetch_all(pool)
            .await
            .context("reconcile fetch failed")?;
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

/// Reconcilia Supabase con la BD local MES POR MES (resumible + circuit breaker).
/// Reemplaza el full-window purge: solo los meses cuyo checksum local difiere del
/// ultimo subido se purgan + re-suben. El estado vive en `sync_months`, asi que si
/// se interrumpe retoma donde quedo en la proxima corrida. NO hace full-scan remoto.
pub async fn reconcile_supabase(
    pool: &sqlx::SqlitePool,
    retention_days: u32,
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

    // 0) Limpieza: eliminar de Supabase los meses FUERA de la ventana de retencion
    //    (mes_ref < cutoff). Necesario al bajar la retencion: la reconciliacion por
    //    si sola no toca los meses fuera de ventana. Best-effort (reintenta luego).
    if let Some(cb) = progress_cb {
        cb(0, 0, 0.02, "Limpiando meses fuera de la ventana de retención...");
    }
    match cleanup_supabase_retention(retention_days, progress_cb).await {
        Ok(n) if n > 0 => {
            info!("Reconcile: retención limpió {} filas fuera de ventana", n);
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Reconcile: limpieza de retención falló (se reintenta): {}", e),
    }

    // 1) Checksums locales ELEGIBLES por mes (mismo filtro del uploader)
    let base_where = if retention_days > 0 {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .format("%Y-%m")
            .to_string();
        format!("WHERE mes_ref >= '{}'", cutoff)
    } else {
        String::new()
    };
    let where_clause = crate::config::with_line_filter(&base_where);
    let local: Vec<(String, String, i64)> = sqlx::query_as(&format!(
        "SELECT mes_ref, printf('%08x-%08x-%08x', COUNT(*), \
                CAST(ROUND(SUM(soles)*100) AS INTEGER), \
                CAST(ROUND(SUM(cantidad)*100) AS INTEGER)) AS checksum, \
                COUNT(*) AS filas \
         FROM ventas {} GROUP BY mes_ref ORDER BY mes_ref",
        where_clause
    ))
    .fetch_all(pool)
    .await
    .context("reconcile: local checksums failed")?;

    // 2) Persistir checksum local (crea filas nuevas con remote_checksum NULL)
    for (mes, checksum, filas) in &local {
        sqlx::query(
            "INSERT INTO sync_months (mes_ref, local_checksum, filas, actualizado_en) \
             VALUES (?, ?, ?, datetime('now')) \
             ON CONFLICT(mes_ref) DO UPDATE SET local_checksum = excluded.local_checksum, \
                filas = excluded.filas, actualizado_en = excluded.actualizado_en",
        )
        .bind(mes)
        .bind(checksum)
        .bind(filas)
        .execute(pool)
        .await?;
    }

    // 3) Sembrar estado desde Supabase cuando la BD local fue recreada (sync_months
    //    perdido). Si un mes ya existe en Supabase con el MISMO conteo local, se marca
    //    como sincronizado SIN re-subir. Solo los meses con diferencia se reconcilian.
    //    (No aplica a reset forzado: reset_sync_marker pone remote_checksum='' , no NULL.)
    let sin_estado: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_months WHERE remote_checksum IS NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if sin_estado > 0 {
        if let Ok(sb_counts) = fetch_supabase_month_counts(&client, &url, &key).await {
            let mut sembrados = 0usize;
            for (mes, _checksum, filas) in &local {
                if sb_counts.get(mes) == Some(filas) {
                    let r = sqlx::query(
                        "UPDATE sync_months SET remote_checksum = local_checksum, estado = 'synced' \
                         WHERE mes_ref = ? AND remote_checksum IS NULL",
                    )
                    .bind(mes)
                    .execute(pool)
                    .await?;
                    sembrados += r.rows_affected() as usize;
                }
            }
            if sembrados > 0 {
                info!(
                    "Reconcile: sembrados {} meses desde Supabase (mismo conteo, sin re-subir)",
                    sembrados
                );
            }
        } else {
            info!("Reconcile: RPC g360_ventas_por_mes no disponible; se omite el sembrado");
        }
    }

    // 4) Meses a sincronizar: local != remoto (o nunca subido)
    let pendientes: Vec<(String, String)> = sqlx::query_as(
        "SELECT mes_ref, local_checksum FROM sync_months \
         WHERE remote_checksum IS NULL OR remote_checksum != local_checksum \
         ORDER BY mes_ref",
    )
    .fetch_all(pool)
    .await
    .context("reconcile: pending months failed")?;

    let months_total = pendientes.len();
    let mut synced = 0usize;
    let mut failed = 0usize;
    let mut rows_uploaded = 0usize;
    let mut rows_purged = 0usize;
    let mut consecutive_failures = 0usize;
    let mut stopped_early = false;

    for (i, (mes, local_checksum)) in pendientes.iter().enumerate() {
        if let Some(cb) = progress_cb {
            let frac = (i as f64 / months_total.max(1) as f64) * 0.98;
            cb(
                0,
                months_total,
                frac as f32,
                &format!("Reconciliando mes {}/{} ({})", i + 1, months_total, mes),
            );
        }
        match reconcile_month(pool, &client, &base_url, &key, mes).await {
            Ok((purged, uploaded)) => {
                sqlx::query(
                    "UPDATE sync_months SET remote_checksum = local_checksum, estado = 'synced', \
                        intentos = 0, ultimo_error = NULL, actualizado_en = datetime('now') \
                     WHERE mes_ref = ?",
                )
                .bind(mes)
                .execute(pool)
                .await?;
                synced += 1;
                rows_purged += purged;
                rows_uploaded += uploaded;
                consecutive_failures = 0;
                info!(
                    "Reconcile {}: purgadas {} + subidas {} (checksum {})",
                    mes, purged, uploaded, local_checksum
                );
            }
            Err(e) => {
                let emsg: String = e.to_string().chars().take(300).collect();
                sqlx::query(
                    "UPDATE sync_months SET estado = 'error', intentos = intentos + 1, \
                        ultimo_error = ?, actualizado_en = datetime('now') WHERE mes_ref = ?",
                )
                .bind(&emsg)
                .bind(mes)
                .execute(pool)
                .await?;
                failed += 1;
                consecutive_failures += 1;
                tracing::warn!("Reconcile {} fallo: {}", mes, emsg);
                if consecutive_failures >= RECONCILE_MAX_CONSECUTIVE_FAILURES {
                    stopped_early = true;
                    info!(
                        "Reconcile: cortado tras {} fallos consecutivos (probable throttling); \
                         se reanuda en la proxima corrida",
                        consecutive_failures
                    );
                    break;
                }
            }
        }
    }

    Ok(ReconcileResult {
        months_total,
        months_synced: synced,
        months_failed: failed,
        rows_uploaded,
        rows_purged,
        stopped_early,
    })
}

/// Enumera los `mes_ref` (YYYY-MM) cubiertos por `[desde, hasta]` (YYYY-MM-DD).
pub fn month_span(desde: &str, hasta: &str) -> Result<Vec<String>> {
    let parse = |s: &str| -> Result<chrono::NaiveDate> {
        let base = if s.len() >= 7 { &s[..7] } else { s };
        chrono::NaiveDate::parse_from_str(&format!("{}-01", base), "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("fecha inválida {s}: {e}"))
    };
    let a = parse(desde)?;
    let b = parse(hasta)?;
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    let mut meses = Vec::new();
    let mut cur = a;
    loop {
        meses.push(cur.format("%Y-%m").to_string());
        if cur >= b {
            break;
        }
        cur = cur
            .checked_add_months(chrono::Months::new(1))
            .ok_or_else(|| anyhow::anyhow!("overflow de meses"))?;
    }
    Ok(meses)
}

/// Checksum local elegible de UN mes (mismo filtro del uploader).
pub async fn local_month_checksum(
    pool: &sqlx::SqlitePool,
    mes: &str,
) -> Result<Option<(String, i64)>> {
    let where_clause =
        crate::config::with_line_filter(&format!("WHERE mes_ref = '{}'", mes.replace('\'', "''")));
    let row: Option<(String, i64)> = sqlx::query_as(&format!(
        "SELECT printf('%08x-%08x-%08x', COUNT(*), \
                CAST(ROUND(SUM(soles)*100) AS INTEGER), \
                CAST(ROUND(SUM(cantidad)*100) AS INTEGER)) AS checksum, COUNT(*) AS filas \
         FROM ventas {} GROUP BY mes_ref",
        where_clause
    ))
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

