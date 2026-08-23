use crate::config::{get_supabase_key, get_supabase_url, SUPABASE_TABLE};
use crate::models::Venta;
use anyhow::{Context, Result};
use tracing::info;

pub async fn upload_to_supabase(ventas: &[Venta]) -> Result<usize> {
    let url = get_supabase_url();
    let key = get_supabase_key();
    if url.contains("TU_SUPABASE") || key.contains("TU_ANON") {
        return Err(anyhow::anyhow!("Supabase credentials not configured"));
    }
    let endpoint = format!(
        "{}/rest/v1/{}?onConflict=doc_std,id_cliente,fecha_orig",
        url, SUPABASE_TABLE
    );
    let body = serde_json::to_string(ventas)?;
    let client = reqwest::Client::new();
    let resp = client
        .post(&endpoint)
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .header("Prefer", "resolution=merge-upsert")
        .body(body)
        .send()
        .await
        .context("Supabase request failed")?;
    let st = resp.status();
    let txt = resp.text().await?;
    info!("Supabase: {} ({} chars)", st, txt.len());
    if st.is_success() {
        Ok(ventas.len())
    } else {
        Err(anyhow::anyhow!("Supabase {}: {}", st, txt))
    }
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
