//! Fecha de última captura con buffer — helper único para el "-1 día".
//!
//! El ERP CIPSA permite postear documentos con retraso (1-2 días). Capturar
//! solo "desde hoy" pierde esos documentos: el punto de partida correcto es
//! `MAX(fecha_orig) - buffer_days`. Todas las rutas de captura (botón manual,
//! pipeline diario, task scheduler) usan esta función para ser consistentes.
//!
//! Nota: `build_pending` (capture.rs) aplica además un piso de 2021-01-01 y
//! decide modo mensual vs diario según el gap — ese control vive ahí; este
//! helper solo resuelve la fecha con buffer.

use anyhow::{Context, Result};
use chrono::Duration as ChronoDuration;

/// `MAX(fecha_orig)` de la BD retrocedido `buffer_days` días (ISO yyyy-mm-dd).
/// - BD vacía → hoy (hora local) − buffer_days.
/// - Fecha no parseable en BD → hoy (hora local) − buffer_days (fallback seguro).
pub async fn last_captured_date(pool: &sqlx::SqlitePool, buffer_days: i64) -> Result<String> {
    let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
        .fetch_one(pool)
        .await
        .context("last_captured_date: MAX(fecha_orig) failed")?;
    let base = last
        .and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| chrono::Local::now().naive_local().date());
    Ok((base - ChronoDuration::days(buffer_days))
        .format("%Y-%m-%d")
        .to_string())
}

/// Buffer estándar de captura (días de retroceso sobre MAX(fecha_orig)).
pub const CAPTURE_BUFFER_DAYS: i64 = 1;
