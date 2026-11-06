use crate::config::{get_supabase_service_key, get_supabase_url, SUPABASE_TABLE};
use anyhow::{Context, Result};
use std::time::Duration;


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
    /// % de uso vs plan free 500 MB (0 si la vista vw_ventas_storage no existe).
    pub pct_of_500mb: f64,
    /// Tamaño total de la tabla (bytes) — 0 si la vista no existe.
    pub total_bytes: i64,
}

/// Cuenta las filas de Supabase con `count=planned` (estimador del planner).
/// NO hace full-scan: es barato en disco, clave cuando el Disk IO Budget esta
/// limitado. El valor es aproximado (suficiente para el display del dashboard).
pub async fn count_supabase_rows(client: &reqwest::Client, base: &str, key: &str) -> Result<i64> {
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
pub async fn fetch_edge_date(
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
pub async fn fetch_supabase_documents(
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
pub async fn fetch_supabase_month_counts(
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
    let (pct_of_500mb, total_bytes) =
        fetch_storage_pct(&client, &url, &key).await.unwrap_or((0.0, 0));
    Ok(SupabaseInfo {
        min_date,
        max_date,
        total_rows,
        total_documents,
        pct_of_500mb,
        total_bytes,
    })
}

/// Lee la vista `vw_ventas_storage` para obtener % de uso vs plan 500 MB.
/// Best-effort: si la vista no existe devuelve (0.0, 0).
pub async fn fetch_storage_pct(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<(f64, i64)> {
    let resp = client
        .get(format!("{}/rest/v1/vw_ventas_storage", url))
        .header("apikey", key)
        .header("Authorization", format!("Bearer {}", key))
        .query(&[("select", "pct_of_500mb,total_bytes"), ("limit", "1")])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("storage view {}", resp.status()));
    }
    let rows: serde_json::Value = resp.json().await?;
    let first = rows.as_array().and_then(|a| a.first());
    match first {
        Some(v) => {
            let pct = v
                .get("pct_of_500mb")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            let bytes = v
                .get("total_bytes")
                .and_then(|x| x.as_i64())
                .unwrap_or(0);
            Ok((pct, bytes))
        }
        None => Ok((0.0, 0)),
    }
}

