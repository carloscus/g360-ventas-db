use crate::config::{get_supabase_service_key, get_supabase_url, SUPABASE_TABLE};
use crate::models::Venta;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

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

/// Columnas de `ventas` en el orden EXACTO que espera `map_venta_row`.
const VENTA_SELECT_COLS: &str = "id, id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,\
id_tipo,nom_tipo,id_familia,nom_familia,\
id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,\
moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,\
fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,\
departamento,provincia,distrito,id_vendedor,\
nom_vendedor,id_pedido,ord_compra,file_source,mes_ref,\
tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico";

/// Mapea una fila de `ventas` (SELECT con el orden de columnas de `VENTA_SELECT_COLS`)
/// a `(id, Venta)`. Reutilizable por upload_all y reconcile_supabase.
fn map_venta_row(row: sqlx::sqlite::SqliteRow) -> (i64, Venta) {
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
        id_ubigeo: row.try_get(45).unwrap_or_default(),
        canal_distribucion: row.try_get(46).unwrap_or_default(),
        nom_condicion_pago: row.try_get(47).unwrap_or_default(),
        estado_linea: row.try_get(48).unwrap_or_default(),
        division: row.try_get(49).unwrap_or_default(),
        fec_cargo: row.try_get(50).unwrap_or_default(),
        id_guia: row.try_get(51).unwrap_or_default(),
    };
    (row.get(0), v)
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

/// Borra todas las filas de un mes (`mes_ref=eq.MES`) en batches por `id`.
/// Un DELETE del mes completo puede exceder el `statement_timeout` de PostgREST
/// (ERROR 57014) cuando el mes tiene decenas de miles de filas o hay bloat; borrar
/// por lotes de ids mantiene cada statement corto y predecible. Devuelve filas borradas.
/// BATCH=2000: el DELETE por PK sigue siendo sub-segundo; reduce a la mitad los
/// round-trips frente a 500 (ver pg_stat_statements: 1729 DELETEs de retencion).
async fn delete_month_batched(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    mes: &str,
) -> Result<usize> {
    const BATCH: usize = 2000;
    let auth = format!("Bearer {}", key);
    let mut deleted = 0usize;
    let mut iter = 0usize;
    loop {
        iter += 1;
        if iter > 10_000 {
            return Err(anyhow::anyhow!(
                "Supabase DELETE ids {}: demasiadas iteraciones (posible bloqueo de borrado)",
                mes
            ));
        }
        let mes_filter = format!("eq.{}", mes);
        let limit_s = BATCH.to_string();

        // 1) Leer hasta BATCH ids del mes (seek por idx_venta_mes)
        let mut ids: Vec<String> = Vec::new();
        let mut last_err: Option<String> = None;
        for attempt in 1..=3 {
            match client
                .get(base_url)
                .header("apikey", key)
                .header("Authorization", &auth)
                .query(&[
                    ("mes_ref", mes_filter.as_str()),
                    ("select", "id"),
                    ("order", "id"),
                    ("limit", limit_s.as_str()),
                ])
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    let rows: serde_json::Value =
                        r.json().await.unwrap_or(serde_json::json!([]));
                    ids = rows
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.get("id").map(|v| v.to_string())).collect())
                        .unwrap_or_default();
                    last_err = None;
                    break;
                }
                Ok(r) => {
                    let st = r.status();
                    if st.is_client_error() {
                        return Err(anyhow::anyhow!("Supabase GET ids {}: {}", mes, st));
                    }
                    last_err = Some(format!("{}", st));
                }
                Err(e) => last_err = Some(format!("red: {e}")),
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
        }
        if let Some(e) = last_err {
            return Err(anyhow::anyhow!("Supabase GET ids {} tras 3 intentos: {}", mes, e));
        }
        if ids.is_empty() {
            break;
        }

        // 2) DELETE por `id=in.(...)`
        let id_filter = format!("in.({})", ids.join(","));
        let mut last_err: Option<String> = None;
        let mut ok = false;
        for attempt in 1..=3 {
            match client
                .delete(base_url)
                .header("apikey", key)
                .header("Authorization", &auth)
                .header("Prefer", "return=minimal")
                .query(&[("id", id_filter.as_str())])
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    ok = true;
                    last_err = None;
                    break;
                }
                Ok(r) => {
                    let st = r.status();
                    if st.is_client_error() {
                        return Err(anyhow::anyhow!("Supabase DELETE ids {}: {}", mes, st));
                    }
                    last_err = Some(format!("{}", st));
                }
                Err(e) => last_err = Some(format!("red: {e}")),
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
        }
        if !ok {
            return Err(anyhow::anyhow!(
                "Supabase DELETE ids {} tras 3 intentos: {}",
                mes,
                last_err.unwrap_or_default()
            ));
        }
        deleted += ids.len();
        // NOTA: NO cortar cuando ids.len() < BATCH. Supabase PostgREST tiene
        // max-rows=1000 por defecto: aunque pidamos limit=BATCH el servidor
        // devuelve ≤1000, y 1000 < BATCH haría creer que el mes está vacío
        // dejando filas viejas (bug: purga parcial -> duplicados ~2x).
        // El unico criterio de "mes vacio" es que el GET devuelva 0 ids
        // (manejado arriba con ids.is_empty()). El guard de iteraciones
        // protege contra un DELETE que no borra nada.
    }
    Ok(deleted)
}

/// Como `delete_month_batched` pero solo borra los días `>= fecha_floor` (YYYY-MM-DD)
/// dentro del mes. Sirve al sync incremental por día: evita borrar/re-subir el mes
/// completo cuando solo cambió un día (menos bloat). Devuelve filas borradas.
async fn delete_days_in_month_batched(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    mes: &str,
    fecha_floor: &str,
) -> Result<usize> {
    const BATCH: usize = 2000;
    let auth = format!("Bearer {}", key);
    let mut deleted = 0usize;
    let mut iter = 0usize;
    loop {
        iter += 1;
        if iter > 10_000 {
            return Err(anyhow::anyhow!(
                "Supabase DELETE días {} >= {}: demasiadas iteraciones",
                mes, fecha_floor
            ));
        }
        let mes_filter = format!("eq.{}", mes);
        let floor_filter = format!("gte.{}", fecha_floor);
        let limit_s = BATCH.to_string();

        let mut ids: Vec<String> = Vec::new();
        let mut last_err: Option<String> = None;
        for attempt in 1..=3 {
            match client
                .get(base_url)
                .header("apikey", key)
                .header("Authorization", &auth)
                .query(&[
                    ("mes_ref", mes_filter.as_str()),
                    ("fecha_orig", floor_filter.as_str()),
                    ("select", "id"),
                    ("order", "id"),
                    ("limit", limit_s.as_str()),
                ])
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    let rows: serde_json::Value =
                        r.json().await.unwrap_or(serde_json::json!([]));
                    ids = rows
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.get("id").map(|v| v.to_string())).collect())
                        .unwrap_or_default();
                    last_err = None;
                    break;
                }
                Ok(r) => {
                    let st = r.status();
                    if st.is_client_error() {
                        return Err(anyhow::anyhow!("Supabase GET ids {} {}: {}", mes, fecha_floor, st));
                    }
                    last_err = Some(format!("{}", st));
                }
                Err(e) => last_err = Some(format!("red: {e}")),
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
        }
        if let Some(e) = last_err {
            return Err(anyhow::anyhow!("Supabase GET ids {} {} tras 3 intentos: {}", mes, fecha_floor, e));
        }
        if ids.is_empty() {
            break;
        }

        let id_filter = format!("in.({})", ids.join(","));
        let mut last_err: Option<String> = None;
        let mut ok = false;
        for attempt in 1..=3 {
            match client
                .delete(base_url)
                .header("apikey", key)
                .header("Authorization", &auth)
                .header("Prefer", "return=minimal")
                .query(&[("id", id_filter.as_str())])
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    ok = true;
                    last_err = None;
                    break;
                }
                Ok(r) => {
                    let st = r.status();
                    if st.is_client_error() {
                        return Err(anyhow::anyhow!("Supabase DELETE ids {} {}: {}", mes, fecha_floor, st));
                    }
                    last_err = Some(format!("{}", st));
                }
                Err(e) => last_err = Some(format!("red: {e}")),
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
        }
        if !ok {
            return Err(anyhow::anyhow!(
                "Supabase DELETE ids {} {} tras 3 intentos: {}",
                mes, fecha_floor, last_err.unwrap_or_default()
            ));
        }
        deleted += ids.len();
        // Mismo fix que delete_month_batched: no cortar con partial batch
        // (PostgREST max-rows=1000 < BATCH haria cortar prematuramente).
        // Criterio unico de fin: GET devolvio 0 ids (arriba).
    }
    Ok(deleted)
}

/// Purga la ventana de retencion en Supabase (full sync), MES POR MES.
/// El DELETE se hace por lotes de ids (ver `delete_month_batched`): un DELETE del
/// mes completo excede el statement_timeout de PostgREST (57014) en meses grandes.
/// Reintento por lote ante 5xx transitorios.
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
        match delete_month_batched(&client, &base_url, &key, mes).await {
            Ok(n) => {
                purged += n;
                info!("Purge {}: {} filas (acumulado {})", mes, n, purged);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Full sync: fallo purge de ventana: {}", e));
            }
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
        // Borrado por lotes de ids (best-effort: la limpieza fuera de ventana no
        // debe abortar el upload si un mes falla).
        match delete_month_batched(&client, &base_url, &key, mes).await {
            Ok(n) => {
                deleted += n;
                if let Some(cb) = progress_cb {
                    let frac = (mi + 1) as f64 / meses_purgar.len().max(1) as f64;
                    cb(0, 0, (0.95 + frac * 0.05) as f32, &format!(
                        "Retencion: mes {} limpiado ({} acumulados)",
                        mes, deleted
                    ));
                }
            }
            Err(e) => {
                info!("Supabase retention delete fallo ({}): {}", mes, e);
            }
        }
    }

    info!("Supabase retention: {} registros eliminados", deleted);
    Ok(deleted)
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
const RECONCILE_MAX_CONSECUTIVE_FAILURES: usize = 3;

/// Sincroniza UN mes: purga sus filas en Supabase (por lotes de ids) y re-sube
/// las filas elegibles desde SQLite (keyset por id). Devuelve (purgadas, subidas).
async fn reconcile_month(
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
fn month_span(desde: &str, hasta: &str) -> Result<Vec<String>> {
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
async fn local_month_checksum(
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

/// Sincroniza UN mes pero SOLO los días `>= fecha_floor`: purga esos días en
/// Supabase y re-sube las filas locales elegibles de esos días. Devuelve (purgadas, subidas).
async fn sync_month_days(
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

/// Cobertura de Supabase: rango de fechas (`fecha_orig`) y total de filas.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SupabaseInfo {
    pub min_date: String,
    pub max_date: String,
    pub total_rows: i64,
    /// Documentos distintos (COUNT(DISTINCT folio_unico)). None si la funcion
    /// RPC `g360_ventas_stats()` no esta creada en Supabase.
    pub total_documents: Option<i64>,
}

/// Cuenta las filas de Supabase con `count=planned` (estimador del planner).
/// NO hace full-scan: es barato en disco, clave cuando el Disk IO Budget esta
/// limitado. El valor es aproximado (suficiente para el display del dashboard).
async fn count_supabase_rows(client: &reqwest::Client, base: &str, key: &str) -> Result<i64> {
    let resp = client
        .get(base)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Prefer", "count=planned")
        .header("Range", "0-0")
        .query(&[("select", "id")])
        .send()
        .await?;
    Ok(resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|cr| cr.split('/').nth(1))
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0))
}

/// Lee la fecha `fecha_orig` mas antigua (dir="asc") o mas nueva (dir="desc").
async fn fetch_edge_date(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    dir: &str,
) -> Result<String> {
    let order = format!("fecha_orig.{}", dir);
    let resp = client
        .get(base)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .query(&[
            ("select", "fecha_orig"),
            ("order", order.as_str()),
            ("limit", "1"),
            ("fecha_orig", "not.is.null"),
        ])
        .send()
        .await?;
    let rows: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));
    Ok(rows
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("fecha_orig"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Cuenta documentos distintos en Supabase via la funcion RPC `g360_ventas_stats()`.
/// Best-effort: si la funcion no existe (404/PGRST202), el llamador la omite.
async fn fetch_supabase_documents(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<i64> {
    let endpoint = format!("{}/rest/v1/rpc/g360_ventas_stats", url);
    let resp = client
        .post(&endpoint)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("rpc g360_ventas_stats: {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));
    v.as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("documentos"))
        .and_then(|d| d.as_i64())
        .ok_or_else(|| anyhow::anyhow!("rpc sin campo documentos"))
}

/// Conteo de filas por mes en Supabase via RPC `g360_ventas_por_mes()`.
/// Se usa para sembrar `sync_months` cuando la BD local se recrea (sin re-subir).
async fn fetch_supabase_month_counts(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<std::collections::HashMap<String, i64>> {
    let endpoint = format!("{}/rest/v1/rpc/g360_ventas_por_mes", url);
    let resp = client
        .post(&endpoint)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("rpc g360_ventas_por_mes: {}", resp.status());
    }
    let rows: Vec<serde_json::Value> = resp.json().await.unwrap_or_default();
    let mut map = std::collections::HashMap::new();
    for r in rows {
        if let (Some(m), Some(n)) = (
            r.get("mes_ref").and_then(|v| v.as_str()),
            r.get("filas").and_then(|v| v.as_i64()),
        ) {
            map.insert(m.to_string(), n);
        }
    }
    Ok(map)
}

/// Consulta Supabase para mostrar la fecha mas antigua, mas nueva y el total de filas.
/// Lectura liviana: 2 seeks con `limit=1` + 1 `count=planned`. Best-effort para documentos.
pub async fn get_supabase_info() -> Result<SupabaseInfo> {
    let url = get_supabase_url();
    let key = get_supabase_service_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Supabase no configurado"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let base = format!("{}/rest/v1/{}", url, SUPABASE_TABLE);
    let total_rows = count_supabase_rows(&client, &base, &key).await?;
    let min_date = fetch_edge_date(&client, &base, &key, "asc").await.unwrap_or_default();
    let max_date = fetch_edge_date(&client, &base, &key, "desc").await.unwrap_or_default();
    let total_documents = fetch_supabase_documents(&client, &url, &key).await.ok();
    Ok(SupabaseInfo { min_date, max_date, total_rows, total_documents })
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
