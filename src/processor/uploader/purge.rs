use crate::config::{get_supabase_service_key, get_supabase_url, SUPABASE_TABLE};
use anyhow::Result;
use std::time::Duration;
use tracing::info;

use super::*;

/// Borra todas las filas de un mes (`mes_ref=eq.MES`) en batches por `id`.
/// Un DELETE del mes completo puede exceder el `statement_timeout` de PostgREST
/// (ERROR 57014) cuando el mes tiene decenas de miles de filas o hay bloat; borrar
/// por lotes de ids mantiene cada statement corto y predecible. Devuelve filas borradas.
/// BATCH=2000: el DELETE por PK sigue siendo sub-segundo; reduce a la mitad los
/// round-trips frente a 500 (ver pg_stat_statements: 1729 DELETEs de retencion).
pub async fn delete_month_batched(
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
pub async fn delete_days_in_month_batched(
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

