use std::time::{Instant, UNIX_EPOCH};

use g360_db_ventas::capture_state::CapturePhase;

use crate::state::*;
use crate::types::*;

// ── Dashboard y Health ─────────────────────────────────────────────────
#[tauri::command]
pub async fn get_dashboard() -> Result<DashboardStats, String> {
    // Cache TTL: el UI hace polling cada ~2s y la query GROUP BY tarda 4-24s.
    {
        let c = DASHBOARD_CACHE.lock().unwrap();
        if let Some((t, stats)) = c.as_ref() {
            if t.elapsed().as_secs() < DASHBOARD_CACHE_TTL_SECS {
                return Ok(stats.clone());
            }
        }
    }
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    // Stats cache: evita full-scan de 1.1M filas en queries lentas.
    // La columna value es REAL -> leer como f64 y castear (REAL->i64 falla en sqlx).
    let total_records: i64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_records'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as i64;
    let total_sales: f64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_sales'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as f64;
    let total_clients: i64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_clients'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as i64;
    let total_skus: i64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key='total_skus'")
        .fetch_one(&pool).await.map_err(|e| e.to_string()).unwrap_or(0.0) as i64;
    // Agregado precomputado en stats_por_mes (instantaneo). Fallback a calculo en
    // vivo solo si la tabla esta vacia (BD recien creada).
    let mut months: Vec<MonthStats> = sqlx::query_as(
        "SELECT mes_ref, filas AS rows, ventas_soles AS sales, clientes AS clients \
         FROM stats_por_mes ORDER BY mes_ref",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    if months.is_empty() {
        months = sqlx::query_as("SELECT mes_ref, COUNT(*) as rows, COALESCE(SUM(soles), 0.0) as sales, COUNT(DISTINCT id_cliente) as clients FROM ventas GROUP BY mes_ref ORDER BY mes_ref")
            .fetch_all(&pool)
            .await
            .map_err(|e| e.to_string())?;
    }
    let stats = DashboardStats { total_records, total_sales, total_clients, total_skus, months };
    *DASHBOARD_CACHE.lock().unwrap() = Some((Instant::now(), stats.clone()));
    Ok(stats)
}

/// Unix secs -> "YYYY-MM-DD HH:MM" en hora Peru (GMT-5). Formato unificado con el
/// resto de fechas de la UI (ISO).
pub fn fmt_hora_local(secs: u64) -> String {
    // Peru: UTC-5 sin horario de verano
    let secs_pe = secs.saturating_sub(5 * 3600);
    let days = (secs_pe / 86400) as i64;
    let rem = secs_pe % 86400;
    let (h, mi, _s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 { y += 1; }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, h, mi)
}

#[tauri::command]
pub async fn get_health() -> Result<HealthStats, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    // Cache TTL de los conteos crudos (COUNT(*) tarda ~2s; el UI hace polling cada ~2s).
    // Nota: el MutexGuard se dropea explícitamente antes de cualquier await para que el future sea Send.
    let cached = {
        let c = HEALTH_CACHE.lock().unwrap();
        c.as_ref().and_then(|(t, n, mn, mx)| {
            if t.elapsed().as_secs() < HEALTH_CACHE_TTL_SECS {
                Some((*n, mn.clone(), mx.clone()))
            } else {
                None
            }
        })
    };
    let (total_records, min_date_s, max_date_s) = match cached {
        Some(v) => v,
        None => {
            let total_records: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
            let min_date: Option<String> = sqlx::query_scalar("SELECT MIN(fecha_orig) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
            let max_date: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas").fetch_one(&pool).await.map_err(|e| e.to_string())?;
            let v = (total_records, min_date.unwrap_or_default(), max_date.unwrap_or_default());
            *HEALTH_CACHE.lock().unwrap() = Some((Instant::now(), v.0, v.1.clone(), v.2.clone()));
            v
        }
    };
    let db_path = g360_db_ventas::config::db_path();
    let db_exists = db_path.exists();
    let db_size_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1024.0 / 1024.0).unwrap_or(0.0);
    let last_capture = std::fs::metadata(&db_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| fmt_hora_local(d.as_secs()))
        .unwrap_or_else(|| "n/a".into());
    let running = {
        let st = CAPTURE_STATE.lock().unwrap();
        st.phase != CapturePhase::Idle && st.phase != CapturePhase::Done && st.phase != CapturePhase::Error
    };
    let lock_path = g360_db_ventas::config::raw_dir().join("capture.lock");
    let lock_exists = lock_path.exists();
    // Auto-limpiar lock huerfano en health check (no bloquear UI con STALE)
    let stale_lock = if lock_exists && !running {
        if let Ok(meta) = std::fs::metadata(&lock_path) {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed().map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))) {
                if elapsed.as_secs() > 2 {
                    let _ = std::fs::remove_file(&lock_path);
                    false
                } else { true }
            } else { let _ = std::fs::remove_file(&lock_path); false }
        } else { false }
    } else { false };
    let status = if lock_exists && stale_lock {
        "STALE_LOCK".to_string()
    } else if running {
        "SYNCING".to_string()
    } else if total_records == 0 {
        "EMPTY".to_string()
    } else {
        "OK".to_string()
    };
    // Verificar versión de contrato
    let contract_ver = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'contracto_version'"
    ).fetch_one(&pool).await.unwrap_or_else(|_| g360_db_ventas::db::schema::CONTRACTO_VERSION.to_string());
    let contract_compat = contract_ver.parse::<i64>().unwrap_or(0) == g360_db_ventas::db::schema::CONTRACTO_VERSION;
    // Cobertura dimensional
    let dim_cliente: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_cliente").fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_vendedor: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_vendedor").fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_articulo: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_articulo").fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_documento: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_documento").fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_linea: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_linea").fetch_one(&pool).await.unwrap_or(0) as usize;
    let checksum_months: usize = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM mes_checksums").fetch_one(&pool).await.unwrap_or(0) as usize;
    let total_months: usize = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM ventas WHERE length(mes_ref)=7").fetch_one(&pool).await.unwrap_or(0) as usize;
    let fk_on: u8 = sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&pool).await.unwrap_or(0);
    Ok(HealthStats { total_records, min_date: min_date_s, max_date: max_date_s, last_capture, db_size_mb, status, db_path: db_path.to_string_lossy().to_string(), db_exists, contracto_version: contract_ver, contracto_compatibil: contract_compat, dim_cliente, dim_vendedor, dim_articulo, dim_documento, dim_linea, checksum_months, total_months, foreign_keys_on: fk_on == 1 })
}

/// Metricas de la BD local para la seccion "DB Local".
#[tauri::command]
pub async fn get_local_metrics() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    g360_db_ventas::db::writer::local_metrics(&pool)
        .await
        .map_err(|e| e.to_string())
}

/// Retorna coverage health: conteo de dims, checksums y estado de foreign_keys.
#[tauri::command]
pub async fn get_db_health() -> Result<serde_json::Value, String> {
    let pool = g360_db_ventas::db::writer::init_pool().await.map_err(|e| e.to_string())?;
    let dim_cliente: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_cliente")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_vendedor: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_vendedor")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_articulo: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_articulo")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_documento: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_documento")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let dim_linea: usize = sqlx::query_scalar("SELECT COUNT(*) FROM dim_linea")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let checksum_months: usize = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM mes_checksums")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let total_months: usize = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM ventas")
        .fetch_one(&pool).await.unwrap_or(0) as usize;
    let fk_on: u8 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool).await.unwrap_or(0);
    Ok(serde_json::json!({
        "dim_cliente": dim_cliente,
        "dim_vendedor": dim_vendedor,
        "dim_articulo": dim_articulo,
        "dim_documento": dim_documento,
        "dim_linea": dim_linea,
        "checksum_months": checksum_months,
        "total_months": total_months,
        "foreign_keys_on": fk_on == 1
    }))
}

