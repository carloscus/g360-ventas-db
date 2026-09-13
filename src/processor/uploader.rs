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

    // Todas las purgas se ejecutan después de validar el lote local; un error de
    // calidad nunca debe dejar Supabase parcialmente vacío.
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

    // VALIDACIÓN duplicados: SOLO entre las filas elegibles para upload y con folio
    // no vacío. Raw-first: un duplicado en filas no-elegibles es irrelevante para
    // Supabase, y folios vacíos ("TPO//") colisionarían entre sí sin ser el mismo doc.
    let dup_count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM (
            SELECT folio_unico, id_articulo, COUNT(*) as cnt
            FROM ventas {} AND folio_unico != ''
            GROUP BY folio_unico, id_articulo
            HAVING COUNT(*) > 1
        )", upload_where
    ))
    .fetch_one(pool)
    .await
    .context("Duplicate check failed")?;

    if dup_count > 0 {
        return Err(anyhow::anyhow!(
            "Hay {} pares (folio,SKU) duplicados entre las filas a subir. Ejecuta 'Reprocesar raw' primero para deduplicar.",
            dup_count
        ));
    }

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

    let total_batches = ((count as f64) / (bs as f64)).ceil() as usize;
    let mut up = 0usize;

    // KEYSET PAGINATION: WHERE id > last ORDER BY id LIMIT n (costo constante por
    // batch, usa PK). El OFFSET previo era O(n^2): en el batch ~950 tenia que
    // recorrer 475K filas antes de leer las suyas -> la subida se degradaba de
    // ~2 batches/s a minutos por batch (bug real 2026-09-09, RAM 4.5 GB incluida).
    let select_sql = format!(
        "SELECT id, id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,\
         id_tipo,nom_tipo,id_familia,nom_familia,\
         id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,\
         moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,\
         fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,\
         departamento,provincia,distrito,id_vendedor,\
          nom_vendedor,id_pedido,ord_compra,file_source,mes_ref,\
         tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico \
         FROM ventas {} AND id > ? ORDER BY id LIMIT ?", upload_where
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
            .map(|row: sqlx::sqlite::SqliteRow| {
                use sqlx::Row;
                let v = Venta {
                    id_articulo: row.get(1),
                    original_sku: row.get(2),
                    nom_articulo: row.get(3),
                    id_linea: row.get(4),
                    nom_linea: row.get(5),
                    id_grupo: row.get(6),
                    nom_grupo: row.get(7),
                    id_tipo: row.get(8),
                    nom_tipo: row.get(9),
                    id_familia: row.get(10),
                    nom_familia: row.get(11),
                    id_cliente: row.get(12),
                    doc_cliente: row.get(13),
                    nom_cliente: row.get(14),
                    tpo_doc: row.get(15),
                    serie_doc: row.get(16),
                    nro_doc: row.get(17),
                    referencia: row.get(18),
                    moneda: row.get(19),
                    cantidad: row.get(20),
                    cantidad_fae: row.try_get(21).unwrap_or_default(),
                    soles: row.get(22),
                    dolares: row.get(23),
                    precio_unitario: row.get(24),
                    anho: row.get(25),
                    mes: row.get(26),
                    fecha_orig: row.get(27),
                    fecha_ref: row.try_get(28).unwrap_or(None),
                    fecha_venc: row.try_get(29).unwrap_or(None),
                    cod_sucursal: row.get(30),
                    nom_sucursal: row.get(31),
                    departamento: row.get(32),
                    provincia: row.get(33),
                    distrito: row.get(34),
                    id_vendedor: row.get(35),
                    nom_vendedor: row.get(36),
                    id_pedido: row.get(37),
                    ord_compra: row.try_get(38).unwrap_or_default(),
                    file_source: row.get(39),
                    mes_ref: row.get(40),
                    tipo_operacion: row.try_get(41).unwrap_or_default(),
                    factura_ref_serie: row.try_get(42).unwrap_or_default(),
                    factura_ref_nro: row.try_get(43).unwrap_or_default(),
                    folio_unico: row.try_get(44).unwrap_or_default(),
                };
                (row.get(0), v)
            })
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
        // MAX(capturado_en) del mes en Supabase
        let resp = client
            .get(&base_url)
            .header("apikey", &key)
            .header("Authorization", format!("Bearer {}", key))
            .header("Prefer", "count=exact")
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

        // content-range total: si 0, el mes no existe en Supabase (mes nuevo, no purgar)
        let total: usize = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|cr| cr.split('/').nth(1).and_then(|s| s.parse().ok()))
            .unwrap_or(0);
        if total == 0 {
            continue; // mes nuevo en Supabase: no requiere purga
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

        // Purgar el mes desactualizado (loop hasta vaciar)
        let mut purged_mes = 0usize;
        loop {
            let resp = client
                .delete(&base_url)
                .header("apikey", &key)
                .header("Authorization", format!("Bearer {}", key))
                .header("Prefer", "return=representation")
                .query(&[("mes_ref", mes_filter.as_str()), ("select", "id")])
                .send()
                .await
                .context("Month-aware: fallo DELETE de mes")?;
            if !resp.status().is_success() {
                anyhow::bail!(
                    "Month-aware: Supabase {} al purgar mes {}: {}",
                    resp.status(), mes_ref,
                    resp.text().await.unwrap_or_default().chars().take(120).collect::<String>()
                );
            }
            let rows: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));
            let n = rows.as_array().map(|a| a.len()).unwrap_or(0);
            purged_mes += n;
            if n == 0 {
                break;
            }
        }
        if purged_mes > 0 {
            info!("Month-aware: mes {} desactualizado -> purgadas {} filas", mes_ref, purged_mes);
            purgados.push((mes_ref.clone(), purged_mes));
        }
    }

    Ok(purgados)
}

/// Purga la ventana de retencion en Supabase (full sync), MES POR MES.
/// Un DELETE unico de toda la ventana (~600K filas) excede el statement_timeout
/// de PostgREST (57014) en tablas con churn/bloat — bug real 2026-09-09 que dejo
/// la purga a medias y Supabase mezclado. Cada mes son ~10-25K filas: seguro.
/// Reintento por mes ante 5xx transitorios.
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
    let cutoff_mes = cutoff_date[..7].to_string();
    let current = chrono::Utc::now().format("%Y-%m").to_string();

    // Lista de meses [cutoff_mes .. current]
    let parse_mes = |m: &str| -> Result<chrono::NaiveDate> {
        Ok(chrono::NaiveDate::from_ymd_opt(
            m[0..4].parse::<i32>()?, m[5..7].parse::<u32>()?, 1,
        ).ok_or_else(|| anyhow::anyhow!("mes invalido: {m}"))?)
    };
    let mut meses: Vec<String> = Vec::new();
    let mut cur = parse_mes(&cutoff_mes)?;
    let end = parse_mes(&current)?;
    while cur <= end {
        meses.push(cur.format("%Y-%m").to_string());
        cur = cur
            .checked_add_months(chrono::Months::new(1))
            .ok_or_else(|| anyhow::anyhow!("mes invalido"))?;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);
    let mut purged = 0usize;

    for mes in &meses {
        loop {
            let mut last_err: Option<String> = None;
            let mut n = 0usize;
            for attempt in 1..=3 {
                let resp = client
                    .delete(&base_url)
                    .header("apikey", &key)
                    .header("Authorization", format!("Bearer {}", key))
                    .header("Prefer", "return=representation")
                    .query(&[("mes_ref", format!("eq.{}", mes).as_str()), ("select", "id")])
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {
                        let rows: serde_json::Value =
                            r.json().await.unwrap_or(serde_json::json!([]));
                        n = rows.as_array().map(|a| a.len()).unwrap_or(0);
                        last_err = None;
                        break;
                    }
                    Ok(r) => {
                        let st = r.status();
                        let txt = r.text().await.unwrap_or_default();
                        let msg = format!("Supabase {}: {}", st, txt.chars().take(120).collect::<String>());
                        // 4xx (no transitorio) -> abortar sin reintentar
                        if st.is_client_error() {
                            return Err(anyhow::anyhow!("Supabase purge {}: {}", mes, msg));
                        }
                        last_err = Some(msg);
                    }
                    Err(e) => last_err = Some(format!("red: {e}")),
                }
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
                }
            }
            if let Some(msg) = last_err {
                return Err(anyhow::anyhow!("Supabase purge {} tras 3 intentos: {}", mes, msg));
            }
            purged += n;
            if n == 0 {
                break; // mes vacio (o ya purgado) -> siguiente mes
            }
        }
        info!("Purge {}: acumulado {} filas", mes, purged);
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

    let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
        .format("%Y-%m-%d")
        .to_string();
    // mes alineado: borrar por mes_ref evita borrar dias sueltos del mes-corte
    let cutoff_mes = cutoff[..7].to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let base_url = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);

    // Chequeo barato de existencia (SIN count=exact: ese count es full-scan y
    // daba 500/57014 en tabla con churn). Con idx_venta_mes, limit=1 es un seek.
    let any_old = match client
        .get(&base_url)
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Range", "0-0")
        .query(&[("mes_ref", format!("lt.{}", cutoff_mes).as_str()), ("select", "id")])
        .send()
        .await
    {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => v.as_array().map(|a| !a.is_empty()).unwrap_or(false),
            Err(_) => false,
        },
        Err(_) => false,
    };

    if !any_old {
        info!("Supabase retention: nada fuera de ventana (< {})", cutoff);
        return Ok(0);
    }

    if let Some(cb) = progress_cb {
        cb(0, 0, 0.95, "Limpiando registros antiguos (fuera de ventana)...");
    }

    info!("Supabase retention: hay registros fuera de ventana (< {}), eliminando por mes...", cutoff);

    // DELETE mes por mes (cada mes es un request pequeño que no excede el
    // statement_timeout de PostgREST; el DELETE 'lt cutoff' unico sí lo hacia)
    let mut deleted = 0usize;

    // Lista de meses [2018-01 .. cutoff_mes)
    let parse_mes = |m: &str| -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::from_ymd_opt(m[0..4].parse::<i32>().ok()?, m[5..7].parse::<u32>().ok()?, 1)
    };
    let mut meses_purgar: Vec<String> = Vec::new();
    let mut cur = chrono::NaiveDate::from_ymd_opt(2018, 1, 1).unwrap(); // min historico del ERP
    if let Some(end) = parse_mes(&cutoff_mes) {
        while cur < end {
            meses_purgar.push(cur.format("%Y-%m").to_string());
            match cur.checked_add_months(chrono::Months::new(1)) {
                Some(d) => cur = d,
                None => break,
            }
        }
    }

    for (mi, mes) in meses_purgar.iter().enumerate() {
        loop {
            let resp = client
                .delete(&base_url)
                .header("apikey", &key)
                .header("Authorization", format!("Bearer {}", key))
                .header("Prefer", "return=representation")
                .query(&[("mes_ref", format!("eq.{}", mes).as_str()), ("select", "id")])
                .send()
                .await;

            if let Err(e) = resp {
                info!("Supabase retention delete red fallo ({}): {}", mes, e);
                break;
            }
            let resp = resp.unwrap();
            if !resp.status().is_success() {
                let txt = resp.text().await.unwrap_or_default();
                info!("Supabase retention delete fallo {}: {}", mes, txt.chars().take(120).collect::<String>());
                break;
            }
            let rows: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));
            let n = rows.as_array().map(|a| a.len()).unwrap_or(0);
            deleted += n;

            if let Some(cb) = progress_cb {
                let frac = (mi + 1) as f64 / meses_purgar.len().max(1) as f64;
                cb(0, 0, (0.95 + frac * 0.05) as f32, &format!(
                    "Retencion: mes {} limpiado ({} acumulados)",
                    mes, deleted
                ));
            }

            if n == 0 {
                break;
            }
        }
    }

    info!("Supabase retention: {} registros eliminados", deleted);
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
            .format("%Y-%m")
            .to_string())
    } else {
        None
    };
    
    // Filtro incremental
    let sync_filter = match last_sync {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    };
    
    // WHERE base: retención + sync incremental
    let base_where = match (&cutoff, &sync_filter) {
        (Some(c), Some(s)) => format!("WHERE mes_ref >= '{}' AND capturado_en > '{}'", c, s),
        (Some(c), None) => format!("WHERE mes_ref >= '{}'", c),
        (None, Some(s)) => format!("WHERE capturado_en > '{}'", s),
        (None, None) => String::new(),
    };

    // Contar filas que se subirian (con el filtro obligatorio de lineas, raw-first)
    let count_query = format!(
        "SELECT COUNT(*) FROM ventas {}",
        crate::config::with_line_filter(&base_where)
    );
    
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
    let invalid_dates: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM ventas {} AND (
            fecha_orig = '0001-01-01'
            OR date(fecha_orig) IS NULL
            OR length(fecha_orig) != 10
            OR substr(fecha_orig, 5, 1) != '-'
            OR substr(fecha_orig, 8, 1) != '-'
            OR substr(fecha_orig, 1, 7) != substr(mes_ref, 1, 7)
        )",
        crate::config::with_line_filter(&base_where)
    ))
    .fetch_one(pool)
    .await?;
    if invalid_dates > 0 {
        invalid_rows.push(format!("fechas_invalidas: {} registros", invalid_dates));
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
