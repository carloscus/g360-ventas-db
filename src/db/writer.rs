// Motor de escritura para g360-ventas-db.
// Todas las funciones criticals para el flujo de cartuchos estan aqui.

use crate::config::db_path;
use crate::models::Venta;
use crate::db::schema::{
    CREATE_INDEXES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL, CREATE_STATS_CACHE_SQL,
    CREATE_AUDIT_TABLES_SQL, CREATE_DIM_TABLES_SQL,
    CONTRACTO_VERSION, DIM_SCHEMA_VERSION, NORMALIZACION_VERSION,
};
use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::sync::Mutex;
use std::io::Read;
use tracing::info;

/// Columnas del INSERT de `ventas` (mismo orden que los binds de `insert_ventas`).
const INSERT_COLS: &str = "id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,id_tipo,nom_tipo,id_familia,nom_familia,id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,departamento,provincia,distrito,id_vendedor,nom_vendedor,id_pedido,ord_compra,file_source,mes_ref,tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico,id_ubigeo,canal_distribucion,nom_condicion_pago,estado_linea,division,fec_cargo,id_guia";

/// Pool singleton: evita recrear la conexión en cada llamada a init_pool().
static POOL: Mutex<Option<SqlitePool>> = Mutex::new(None);

/// Cierra el pool singleton y lo deja para recrear.
pub async fn reset_pool() {
    let taken = { POOL.lock().unwrap().take() };
    if let Some(pool) = taken {
        pool.close().await;
    }
}

/// Inicializa la BD: crea tablas si no existen, migra columnas nuevas,
/// siembra metadata de versiones, reconstruye dims si es necesario.
pub async fn init_pool() -> Result<SqlitePool> {
    {
        let guard = POOL.lock().unwrap();
        if let Some(pool) = guard.as_ref() {
            if !pool.is_closed() {
                return Ok(pool.clone());
            }
        }
    }
    let db = db_path();
    if let Some(p) = db.parent() {
        std::fs::create_dir_all(p)?;
    }
    let db_str = db.to_string_lossy().replace("\\", "/");
    let url = format!("sqlite://{}?mode=rwc", db_str);
    tracing::info!("Connecting to DB: {}", url);
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(120))
        .pragma("synchronous", "NORMAL")
        .pragma("temp_store", "MEMORY")
        .pragma("cache_size", "-64000")
        .pragma("mmap_size", "268435456")
        .pragma("wal_autocheckpoint", "2000")
        .pragma("journal_size_limit", "67108864");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(32)
        .idle_timeout(std::time::Duration::from_secs(60))
        .max_lifetime(std::time::Duration::from_secs(600))
        .connect_with(opts)
        .await
        .context("DB connect failed")?;
    sqlx::query("PRAGMA foreign_keys = ON").execute(&pool).await.ok();
    sqlx::query("PRAGMA optimize").execute(&pool).await.ok();
    *POOL.lock().unwrap() = Some(pool.clone());

    // Crear tabla principal
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await?;

    // Migrar columnas nuevas (seguro, no rompe si ya existe)
    for col in [
        "doc_cliente TEXT", "precio_unitario REAL", "cantidad_fae REAL",
        "original_sku TEXT", "tipo_operacion TEXT DEFAULT 'venta'",
        "factura_ref_serie TEXT", "factura_ref_nro TEXT", "folio_unico TEXT",
        "ord_compra TEXT", "id_ubigeo TEXT", "canal_distribucion TEXT",
        "nom_condicion_pago TEXT", "estado_linea TEXT", "division TEXT",
        "fec_cargo TEXT", "id_guia TEXT",
    ] {
        let name = col.split_whitespace().next().unwrap();
        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('ventas') WHERE name=?")
            .bind(name).fetch_one(&pool).await.unwrap_or(0) > 0;
        if !exists {
            let _ = sqlx::query(&format!("ALTER TABLE ventas ADD COLUMN {}", col)).execute(&pool).await;
        }
    }

    // Crear indices y vistas
    for idx in CREATE_INDEXES_SQL {
        let _ = sqlx::query(idx).execute(&pool).await;
    }
    for v in CREATE_VIEWS_SQL {
        let _ = sqlx::query(v).execute(&pool).await;
    }

    // Crear tablas de audit/estata
    sqlx::query(CREATE_STATS_CACHE_SQL).execute(&pool).await?;
    sqlx::query(CREATE_AUDIT_TABLES_SQL).execute(&pool).await?;
    sqlx::query(CREATE_DIM_TABLES_SQL).execute(&pool).await?;

    // Sembrar metadata de versiones en g360_meta (primera vez)
    sqlx::query(r#"
        INSERT OR REPLACE INTO g360_meta (key, value, actualizado_en)
        VALUES
            ('contracto_version', cast(? as text), datetime('now')),
            ('dim_schema_version', cast(? as text), datetime('now')),
            ('normalizacion_version', cast(? as text), datetime('now'))
    "#)
    .bind(CONTRACTO_VERSION)
    .bind(DIM_SCHEMA_VERSION)
    .bind(NORMALIZACION_VERSION)
    .execute(&pool)
    .await?;

    // stats_cache NO se refresca aqui: la tabla es write-only (ningun SELECT
    // en el repo; el dashboard consulta en vivo). Los refresh post-captura
    // siguen en capture.rs y reparse.rs. Refrescarlo en cada init costaba
    // ~114 s de agregados sobre 2.8M filas.
    let _ = ensure_dims_fresh(&pool).await;

    // Migraciones de reparacion de datos
    // 1. Repoblar precio_unitario donde sea NULL
    sqlx::query("UPDATE ventas SET precio_unitario = CASE WHEN cantidad != 0 THEN soles / cantidad ELSE 0 END WHERE precio_unitario IS NULL").execute(&pool).await?;
    sqlx::query("UPDATE ventas SET precio_unitario = ROUND(soles / cantidad_fae, 4) WHERE cantidad = 0 AND cantidad_fae != 0 AND precio_unitario = 0").execute(&pool).await?;

    // 2. Aplicar CHECKs de integridad en DB de archivo (produccion)
    //    No se aplican en tests in-memory (date('now') no es determinista)
    let is_memory = db.to_string_lossy().contains(":memory:");
    if !is_memory {
        sqlx::query("ALTER TABLE ventas ADD CHECK (length(mes_ref) = 7)").execute(&pool).await.ok();
        sqlx::query("ALTER TABLE ventas ADD CHECK (fecha_orig <= date('now'))").execute(&pool).await.ok();
        sqlx::query("ALTER TABLE ventas ADD CHECK (cantidad >= 0)").execute(&pool).await.ok();
        sqlx::query("ALTER TABLE ventas ADD CHECK (soles >= 0)").execute(&pool).await.ok();
    }

    // 3. Normalizaciones LEGACY (id_linea, id_vendedor, id_pedido, mes_ref):
    //    el parser (desde v1) ya normaliza al insertar, asi que estos UPDATEs
    //    solo reparan filas antiguas (hoy: 0 filas afectadas). Correrlos en
    //    cada arranque escaneaba la tabla completa (~2 min sobre 2.8M filas).
    //    Ahora se ejecutan solo si cambio NORMALIZACION_VERSION o es la
    //    primera vez en esta DB. Fail-open: si algo falla, no se estampa y el
    //    proximo init reintenta.
    let repair_ver: Option<i64> = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'repair_norm_version'",
    )
    .fetch_optional(&pool)
    .await
    .ok()
    .flatten()
    .and_then(|s: String| s.parse().ok());
    if repair_ver != Some(NORMALIZACION_VERSION) {
        let mut repairs_ok = true;

        // id_linea: quitar prefijo "01"
        if let Ok(affected) = sqlx::query("UPDATE ventas SET id_linea = SUBSTR(id_linea, 3) WHERE id_linea GLOB '01??'")
            .execute(&pool).await
        {
            if affected.rows_affected() > 0 {
                info!("Migracion id_linea: {} filas normalizadas (quitando prefijo 01)", affected.rows_affected());
                let _ = refresh_dim_tables(&pool).await;
            }
        } else {
            repairs_ok = false;
        }

        // id_vendedor: quitar prefijo "01"
        if let Ok(affected) = sqlx::query("UPDATE ventas SET id_vendedor = SUBSTR(id_vendedor, 4) WHERE id_vendedor GLOB '01???'")
            .execute(&pool).await
        {
            if affected.rows_affected() > 0 {
                info!("Migracion id_vendedor: {} filas normalizadas (quitando prefijo 01)", affected.rows_affected());
                let _ = refresh_dim_tables(&pool).await;
            }
        } else {
            repairs_ok = false;
        }

        // id_pedido: espacios -> guion_bajo (el parser ya lo hace desde v1)
        if let Ok(affected) = sqlx::query("UPDATE ventas SET id_pedido = REPLACE(id_pedido, ' ', '_') WHERE id_pedido LIKE '% %'")
            .execute(&pool).await
        {
            if affected.rows_affected() > 0 {
                info!("Migracion id_pedido: {} filas normalizadas (espacios -> underscore)", affected.rows_affected());
            }
        } else {
            repairs_ok = false;
        }

        // mes_ref, anho, mes derivados de fecha_orig
        if let Ok(affected) = sqlx::query("UPDATE ventas SET mes_ref = strftime('%Y-%m', fecha_orig), anho = CAST(strftime('%Y', fecha_orig) AS INTEGER), mes = CAST(strftime('%m', fecha_orig) AS INTEGER) WHERE length(mes_ref) != 7 OR length(anho) != 4")
            .execute(&pool).await
        {
            if affected.rows_affected() > 0 {
                info!("Migracion mes_ref/anho/mes: {} filas actualizadas desde fecha_orig", affected.rows_affected());
            }
        } else {
            repairs_ok = false;
        }

        if repairs_ok {
            let _ = sqlx::query(
                "INSERT OR REPLACE INTO g360_meta (key, value, actualizado_en) VALUES ('repair_norm_version', cast(? as text), datetime('now'))",
            )
            .bind(NORMALIZACION_VERSION)
            .execute(&pool)
            .await;
            info!("Normalizaciones legacy aplicadas (normalizacion v{})", NORMALIZACION_VERSION);
        } else {
            info!("Normalizaciones legacy incompletas: sin estampa, se reintenta en el proximo init");
        }
    } else {
        info!("Normalizaciones legacy omitidas (normalizacion v{} ya aplicada)", NORMALIZACION_VERSION);
    }

    info!("DB ready: {} (v{} contract, v{} dim, v{} normalizacion)", db.display(), CONTRACTO_VERSION, DIM_SCHEMA_VERSION, NORMALIZACION_VERSION);
    Ok(pool)
}

/// Verifica que la BD cumpla con el contrato esperado.
/// Los consumidores leen esta funcion antes de usar la DB.
pub async fn check_contract_version(pool: &SqlitePool) -> Result<()> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'contracto_version'"
    ).fetch_optional(pool).await?;
    match stored {
        Some(s) => {
            let version: i64 = s.parse().context("g360_meta.contracto_version no es numerico")?;
            if version != CONTRACTO_VERSION {
                return Err(anyhow::anyhow!(
                    "Versión de contrato incompatibles: DB tiene v{}, esperado v{}. \
                     Actualiza la DB corriendo capture completo o solicita un cartucho nuevo.",
                    version, CONTRACTO_VERSION
                ));
            }
            Ok(())
        }
        None => Err(anyhow::anyhow!(
            "g360_meta no existe o está vacía: la DB no tiene metadata de versión. \
             Ejecuta init_pool() completo o importa un cartucho compatible."
        )),
    }
}

// ─── Insert/Update ───────────────────────────────────────────────────────────

pub async fn insert_ventas(pool: &SqlitePool, ventas: &[Venta], refresh_stats: bool) -> Result<usize> {
    if ventas.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    if let Some(first) = ventas.first() {
        let is_daily = first.mes_ref.len() == 10 && first.mes_ref.chars().filter(|c| *c == '-').count() == 2;
        if is_daily {
            let min_d = ventas.iter().map(|v| v.fecha_orig).min().unwrap_or(first.fecha_orig);
            let max_d = ventas.iter().map(|v| v.fecha_orig).max().unwrap_or(first.fecha_orig);
            sqlx::query("DELETE FROM ventas WHERE fecha_orig BETWEEN ? AND ?")
                .bind(min_d.to_string()).bind(max_d.to_string())
                .execute(&mut *tx).await?;
        } else {
            let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref).fetch_one(&mut *tx).await?;
            if existing > 1000 && (ventas.len() as f64) < (existing as f64) * 0.5 {
                return Err(anyhow::anyhow!(
                    "Guard de integridad: {} filas nuevas < 50% de {} existentes en '{}'. No se reemplaza el mes.",
                    ventas.len(), existing, first.mes_ref
                ));
            }
            sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref).execute(&mut *tx).await?;
        }
    }
    let mut n = 0usize;
    for chunk in ventas.chunks(400) {
        let mut qb = sqlx::QueryBuilder::new(format!("INSERT INTO ventas ({}) ", INSERT_COLS));
        qb.push_values(chunk, |mut b, v| {
            b.push_bind(&v.id_articulo).push_bind(&v.original_sku).push_bind(&v.nom_articulo)
             .push_bind(&v.id_linea).push_bind(&v.nom_linea).push_bind(&v.id_grupo).push_bind(&v.nom_grupo)
             .push_bind(&v.id_tipo).push_bind(&v.nom_tipo).push_bind(&v.id_familia).push_bind(&v.nom_familia)
             .push_bind(&v.id_cliente).push_bind(&v.doc_cliente).push_bind(&v.nom_cliente)
             .push_bind(&v.tpo_doc).push_bind(&v.serie_doc).push_bind(&v.nro_doc).push_bind(&v.referencia)
             .push_bind(&v.moneda).push_bind(v.cantidad).push_bind(v.cantidad_fae).push_bind(v.soles)
             .push_bind(v.dolares).push_bind(v.precio_unitario)
             .push_bind(v.anho).push_bind(v.mes).push_bind(v.fecha_orig.to_string())
             .push_bind(&v.fecha_ref).push_bind(&v.fecha_venc)
             .push_bind(&v.cod_sucursal).push_bind(&v.nom_sucursal).push_bind(&v.departamento)
             .push_bind(&v.provincia).push_bind(&v.distrito)
             .push_bind(&v.id_vendedor).push_bind(&v.nom_vendedor).push_bind(&v.id_pedido).push_bind(&v.ord_compra)
             .push_bind(&v.file_source).push_bind(&v.mes_ref)
             .push_bind(&v.tipo_operacion).push_bind(&v.factura_ref_serie).push_bind(&v.factura_ref_nro).push_bind(&v.folio_unico)
             .push_bind(&v.id_ubigeo).push_bind(&v.canal_distribucion).push_bind(&v.nom_condicion_pago)
             .push_bind(&v.estado_linea).push_bind(&v.division).push_bind(&v.fec_cargo).push_bind(&v.id_guia);
        });
        qb.build().execute(&mut *tx).await?;
        n += chunk.len();
    }
    tx.commit().await?;
    info!("Inserted {} rows", n);
    if refresh_stats {
        let _ = refresh_stats_cache(pool).await;
    }
    Ok(n)
}

// ─── Checksums ───────────────────────────────────────────────────────────────

pub async fn calculate_monthly_checksums(pool: &SqlitePool) -> Result<usize> {
    // Usar query_scalar (no query_as) porque RETURNING devuelve una sola columna
    // y String no implementa FromRow directamente.
    let results: Vec<String> = sqlx::query_scalar(r#"
        INSERT INTO mes_checksums (mes_ref, checksum, total_filas, total_soles, total_cantidad, calculado_en)
        SELECT
            mes_ref,
            printf('%08x-%08x-%08x-%08x',
                COUNT(*),
                CAST(ROUND(SUM(soles) * 100) AS INTEGER) & 0xFFFFFFFF,
                CAST(ROUND(SUM(COALESCE(dolares, 0) * 100), 0) AS INTEGER) & 0xFFFFFFFF,
                CAST(ROUND(SUM(cantidad) * 100) AS INTEGER) & 0xFFFFFFFF
            ) as checksum,
            COUNT(*) as total_filas,
            ROUND(SUM(soles), 2) as total_soles,
            ROUND(SUM(cantidad), 4) as total_cantidad,
            datetime('now')
        FROM ventas
        GROUP BY mes_ref
        ON CONFLICT(mes_ref) DO UPDATE SET
            checksum = excluded.checksum,
            total_filas = excluded.total_filas,
            total_soles = excluded.total_soles,
            total_cantidad = excluded.total_cantidad,
            calculado_en = excluded.calculado_en
        RETURNING mes_ref
    "#).fetch_all(pool).await?;
    Ok(results.len())
}

/// Calcula y persiste checksums diarios en `day_checksums`.
/// Usa INSERT OR REPLACE en vez de ON CONFLICT porque SQLite in-memory
/// rechaza `datetime('now')` no determinista en el contexto de un índice
/// PRIMARY KEY durante ON CONFLICT DO UPDATE.
pub async fn calculate_day_checksums(pool: &SqlitePool, timestamp: &str) -> Result<usize> {
    // Primero limpiar y re-insertar (más simple que ON CONFLICT en in-memory)
    sqlx::query("DELETE FROM day_checksums").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO day_checksums (dia, filas, soles, checksum, calculado_en)
        SELECT
            fecha_orig AS dia,
            COUNT(*) AS filas,
            ROUND(SUM(soles), 2) AS soles,
            COUNT(*) || ':' || ROUND(SUM(soles), 2) AS checksum,
            ? AS calculado_en
        FROM ventas
        WHERE fecha_orig IS NOT NULL AND fecha_orig != ''
        GROUP BY fecha_orig
        RETURNING dia
    "#).bind(timestamp).fetch_all(pool).await?;
    info!("Day checksums calculados: {} días", n.len());
    Ok(n.len())
}

// ─── Export Cartucho ─────────────────────────────────────────────────────────

/// Exporta una INSTANTANEA canonica (cartucho) para que otra app trabaje sobre
/// una copia aislada. Produce:
/// - Archivo .db autocontenido (VACUUM INTO, sin WAL)
/// - Manifiesto JSON con SHA256, versiones, checksums, rango y quality stats
///
/// El consumidor debe:
///   1. Copiar el .db + manifest.json a su directorio de datos
///   2. Opcional: verificar SHA256 del .db contra el manifiesto
///   3. Abrir la DB -> init_pool() detectara versiones automaticamente
///   4. Opcional: check_contract_version() para validar compatibilidad
pub async fn export_canonical_snapshot(target: &std::path::Path, stats: Option<&crate::processor::parser::QualityStats>) -> Result<serde_json::Value> {
    let pool = init_pool().await?;
    let _ = ensure_dims_fresh(&pool).await;
    let _ = recalculate_missing_checksums(&pool).await;
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let _ = calculate_day_checksums(&pool, &ts).await;

    let _ = sqlx::query("VACUUM INTO ?")
        .bind(target.to_string_lossy().to_string())
        .execute(&pool).await?;
    let size = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);

    // SHA256 del archivo exportado (integridad en transporte).
    let mut file = std::fs::File::open(target)?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    let sha256 = format!("{:x}", hasher.finalize());

    let total_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&pool).await.unwrap_or(0);

    let _mm: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT MIN(mes_ref), MAX(mes_ref), COUNT(DISTINCT mes_ref) FROM ventas WHERE length(mes_ref)=7"
    ).fetch_one(&pool).await.unwrap_or((None, None, 0));

    let checksums: Vec<(String, String, i64, f64)> = sqlx::query_as(
        "SELECT mes_ref, checksum, total_filas, total_soles FROM mes_checksums ORDER BY mes_ref"
    ).fetch_all(&pool).await.unwrap_or_default();

    let day_checksums: Vec<(String, i64, f64, String)> = sqlx::query_as(
        "SELECT dia, filas, soles, checksum FROM day_checksums ORDER BY dia"
    ).fetch_all(&pool).await.unwrap_or_default();

    let contract_ver: String = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'contracto_version'"
    ).fetch_one(&pool).await.unwrap_or(CONTRACTO_VERSION.to_string());

    let normalizacion_ver: String = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'normalizacion_version'"
    ).fetch_one(&pool).await.unwrap_or(NORMALIZACION_VERSION.to_string());

    let rango_dias: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT MIN(fecha_orig), MAX(fecha_orig) FROM ventas"
    ).fetch_one(&pool).await.unwrap_or((None, None));

    let rango_desde = rango_dias.0.clone().unwrap_or_default();
    let rango_hasta = rango_dias.1.clone().unwrap_or_default();

    // Determinar estado de precio_unitario
    let precio_recalc_needed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ventas WHERE precio_unitario IS NULL OR precio_unitario = 0"
    ).fetch_one(&pool).await.unwrap_or(0);
    let precio_status = if precio_recalc_needed == 0 { "ok" } else { "needs_recalc" };

    // Determinar estado general de data_quality
    let data_quality = if let Some(s) = stats {
        if s.total_rows > 0 {
            let pct_valid = ((s.total_rows - s.invalid_dates - s.future_dates - s.empty_ids) as f64 / s.total_rows as f64) * 100.0;
            if pct_valid > 99.0 { "excellent".to_string() }
            else if pct_valid > 95.0 { "good".to_string() }
            else { "needs_review".to_string() }
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    let attributes_applied: Vec<&str> = vec![
        "id_pedido_normalized",
        "mes_ref_from_fecha_orig",
        "anio_mes_from_fecha_orig",
        "future_date_filtered",
        "linea_id_normalized",
        "vendedor_id_normalized",
        "cliente_id_normalized",
        "precio_unitario_recalculated",
    ];

    let manifest = serde_json::json!({
        "version": contract_ver.parse::<i64>().unwrap_or(0),
        "contracto_version": contract_ver,
        "dim_schema_version": DIM_SCHEMA_VERSION.to_string(),
        "normalizacion_version": normalizacion_ver,
        "format": "g360-ventas-cartucho-v2",
        "tipo": "semilla",
        "generado_en": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "generado_por": format!("g360-db-ventas/{}", env!("CARGO_PKG_VERSION")),
        "rango": {
            "desde": rango_desde,
            "hasta": rango_hasta
        },
        "allowed_lines": crate::config::effective_allowed_lines(),
        "retention_days": crate::config::load_config().supabase_retention_days_effective(),
        "sha256": sha256,
        "db_size_bytes": size,
        "quick_check": {
            "ok": true,
            "filas": total_rows,
            "soles": checksums.iter().map(|(_, _, _, s)| s).sum::<f64>(),
            "meses": checksums.len()
        },
        "data_quality": {
            "status": data_quality,
            "total_records": stats.map(|s| s.total_rows).unwrap_or(0),
            "valid_records": stats.map(|s| s.total_rows - s.invalid_dates - s.future_dates - s.empty_ids).unwrap_or(0),
            "invalid_records": stats.map(|s| s.invalid_dates + s.future_dates + s.empty_ids).unwrap_or(0),
            "invalid_reasons": {
                "invalid_dates": stats.map(|s| s.invalid_dates).unwrap_or(0),
                "future_dates": stats.map(|s| s.future_dates).unwrap_or(0),
                "empty_ids": stats.map(|s| s.empty_ids).unwrap_or(0)
            },
            "price_unitario_status": precio_status,
            "price_units_recalc_needed": precio_recalc_needed
        },
        "attributes_applied": attributes_applied,
        "dias": day_checksums.iter().map(|(d, _, _, _)| d.clone()).collect::<Vec<_>>(),
        "day_checksums": day_checksums.iter().map(|(d, f, s, c)| {
            serde_json::json!({ d: { "filas": f, "soles": s, "checksum": c } })
        }).collect::<Vec<_>>(),
        "mes_checksums": checksums.iter().map(|(m, c, f, s)| serde_json::json!({
            "mes_ref": m, "checksum": c, "filas": f, "soles": s
        })).collect::<Vec<_>>(),
        "espejo_completo": true,
        "contenido": "raw-first: TODAS las lineas del ERP; sin filtro de lineas ni retencion"
    });

    let manifest_path = target.with_extension("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    info!(
        "Cartucho exportado: {} ({:.1} MB) + manifiesto {}",
        target.display(),
        size as f64 / 1024.0 / 1024.0,
        manifest_path.display()
    );
    // Rotación: conservar solo los N cartuchos más recientes (config.cartucho_keep)
    let keep = crate::config::load_config().cartucho_keep;
    if keep > 0 {
        let _ = prune_cartuchos(keep as usize);
    }

    Ok(serde_json::json!({
        "db_path": target.to_string_lossy().to_string(),
        "manifest_path": manifest_path.to_string_lossy().to_string(),
        "db_size_mb": (size as f64) / 1024.0 / 1024.0,
        "total_rows_raw": total_rows,
        "sha256": sha256,
        "contracto_version": contract_ver,
        "normalizacion_version": normalizacion_ver,
        "dim_schema_version": DIM_SCHEMA_VERSION.to_string(),
        "meses": checksums.len(),
        "dias_con_checksum": day_checksums.len(),
        "rango": { "desde": rango_desde, "hasta": rango_hasta },
        "data_quality": {
            "status": data_quality,
            "valid_records": stats.map(|s| s.total_rows - s.invalid_dates - s.future_dates - s.empty_ids).unwrap_or(0),
            "price_unitario_status": precio_status
        }
    }))
}

/// Recalcula checksums mensuales faltantes.
pub async fn recalculate_missing_checksums(pool: &SqlitePool) -> Result<usize> {
    let n = sqlx::query(r#"
        INSERT INTO mes_checksums (mes_ref, checksum, total_filas, total_soles, calculado_en)
        SELECT
            mes_ref,
            printf('%08x-%08x-%08x-%08x',
                COUNT(*),
                CAST(ROUND(SUM(soles) * 100) AS INTEGER) & 0xFFFFFFFF,
                CAST(ROUND(SUM(COALESCE(dolares, 0) * 100), 0) AS INTEGER) & 0xFFFFFFFF,
                CAST(ROUND(SUM(cantidad) * 100) AS INTEGER) & 0xFFFFFFFF
            ) as checksum,
            COUNT(*) as total_filas,
            ROUND(SUM(soles), 2) as total_soles,
            datetime('now')
        FROM ventas
        WHERE mes_ref NOT IN (SELECT mes_ref FROM mes_checksums)
        GROUP BY mes_ref
        ON CONFLICT(mes_ref) DO UPDATE SET
            checksum = excluded.checksum,
            total_filas = excluded.total_filas,
            total_soles = excluded.total_soles,
            calculado_en = excluded.calculado_en
        RETURNING mes_ref
    "#).fetch_all(pool).await?;
    let count = n.len();
    info!("Checksums mensuales recalculados: {} meses", count);
    Ok(count)
}

// ─── Dims & Stats ────────────────────────────────────────────────────────────

pub async fn refresh_stats_cache(pool: &SqlitePool) -> Result<()> {
    // Los stats del dashboard se calculan con el mismo filtro obligatorio del
    // upload para que reflejen solo lo "vital" (coherentes con Supabase), no el raw.
    let lf = crate::config::with_line_filter("");
    sqlx::query("DELETE FROM stats_cache").execute(pool).await?;
    sqlx::query(&format!(r#"
        INSERT INTO stats_cache (key, value, updated_at)
        VALUES
            ('total_records', (SELECT COUNT(*) FROM ventas {}), datetime('now')),
            ('total_soles', (SELECT ROUND(SUM(soles), 2) FROM ventas {}), datetime('now')),
            ('total_dolares', (SELECT ROUND(SUM(COALESCE(dolares,0)), 2) FROM ventas {}), datetime('now')),
            ('total_clients', (SELECT COUNT(DISTINCT id_cliente) FROM ventas {}), datetime('now')),
            ('total_months', (SELECT COUNT(DISTINCT mes_ref) FROM ventas WHERE length(mes_ref)=7), datetime('now'))
    "#, lf, lf, lf, lf)).execute(pool).await?;
    Ok(())
}

pub async fn refresh_stats_por_mes(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM stats_por_mes").execute(pool).await?;
    sqlx::query(r#"
        INSERT INTO stats_por_mes (mes_ref, filas, ventas_soles, clientes, cantidad, actualizado_en)
        SELECT
            mes_ref,
            COUNT(*) AS filas,
            ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS ventas_soles,
            COUNT(DISTINCT id_cliente) AS clientes,
            ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END), 4) AS cantidad,
            datetime('now')
        FROM ventas
        GROUP BY mes_ref
    "#).execute(pool).await?;
    Ok(())
}

pub async fn refresh_dim_tables(pool: &SqlitePool) -> Result<usize> {
    let n_cli = refresh_dim_cliente(pool).await?;
    let n_vend = refresh_dim_vendedor(pool).await?;
    let n_art = refresh_dim_articulo(pool).await?;
    let n_doc = refresh_dim_documento(pool).await?;
    let n_line = refresh_dim_linea(pool).await?;
    let n_fact = refresh_fact_venta_mes(pool).await?;
    info!("dim_tables rebuild: cliente={}, vendedor={}, articulo={}, documento={}, linea={}, fact_venta_mes={}",
        n_cli, n_vend, n_art, n_doc, n_line, n_fact);
    Ok(n_cli + n_vend + n_art + n_doc + n_line + n_fact)
}

pub async fn ensure_dims_fresh(pool: &SqlitePool) -> Result<usize> {
    let user_version: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(pool).await.unwrap_or(0);
    let dims_vacias: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dim_cliente").fetch_one(pool).await.unwrap_or(0);

    let necesita = dims_vacias == 0 || user_version < DIM_SCHEMA_VERSION;
    if !necesita {
        return Ok(0);
    }

    let n = refresh_dim_tables(pool).await?;
    let _ = sqlx::query(&format!("PRAGMA user_version = {}", DIM_SCHEMA_VERSION)).execute(pool).await;
    if user_version < DIM_SCHEMA_VERSION && dims_vacias > 0 {
        info!("dim_tables migradas a version {} (antes {}): {} filas", DIM_SCHEMA_VERSION, user_version, n);
    }
    Ok(n)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub async fn count_ventas(pool: &SqlitePool) -> Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(pool).await.context("Count query failed")
}

pub async fn count_by_month(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    sqlx::query_as("SELECT mes_ref, COUNT(*) FROM ventas WHERE length(mes_ref)=7 GROUP BY mes_ref ORDER BY mes_ref")
        .fetch_all(pool).await.context("Count by month failed")
}

pub async fn verify_deploy(pool: &SqlitePool) -> Result<Vec<String>> {
    let mut issues = Vec::new();
    let tables = ["ventas", "dim_ruc", "mes_checksums", "sync_log", "audit_log"];
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        )).bind(table).fetch_one(pool).await?;
        if count == 0 {
            issues.push(format!("❌ Tabla critica faltante: {}", table));
        }
    }
    let col_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('ventas')").fetch_one(pool).await?;
    if col_count < 45 {
        issues.push(format!("⚠️ Tabla ventas incompleta: {} columnas (esperado 50+)", col_count));
    }
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(pool).await?;
    if row_count == 0 {
        issues.push("ℹ️ BD vacía: necesita captura inicial".to_string());
    }
    if issues.is_empty() {
        issues.push("✅ Deploy verificado correctamente".to_string());
    }
    Ok(issues)
}

// ─── Impl de dims (para no saturar el archivo principal) ────────────────────

async fn refresh_dim_cliente(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_cliente").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO dim_cliente (id_cliente, nom_cliente, doc_cliente, departamento, provincia, distrito,
                                 n_ventas, total_soles, total_dolares, ultima_compra, actualizado_en)
        SELECT id_cliente, nom_cliente, doc_cliente, departamento, provincia, distrito,
               COUNT(*) AS n_ventas,
               ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS total_soles,
               ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN dolares ELSE 0 END), 2) AS total_dolares,
               MAX(fecha_orig) AS ultima_compra,
               datetime('now')
        FROM ventas
        GROUP BY id_cliente
    "#).execute(pool).await?;
    Ok(n.rows_affected() as usize)
}

async fn refresh_dim_vendedor(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_vendedor").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO dim_vendedor (id_vendedor, nom_vendedor, n_ventas, total_soles, n_clientes, ultima_venta, actualizado_en)
        SELECT id_vendedor, nom_vendedor,
               COUNT(*) AS n_ventas,
               ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS total_soles,
               COUNT(DISTINCT id_cliente) AS n_clientes,
               MAX(fecha_orig) AS ultima_venta,
               datetime('now')
        FROM ventas
        GROUP BY id_vendedor
    "#).execute(pool).await?;
    Ok(n.rows_affected() as usize)
}

async fn refresh_dim_articulo(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_articulo").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO dim_articulo (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
                                  n_ventas, total_cantidad, total_soles, precio_promedio, ultima_venta, actualizado_en)
        SELECT id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
               COUNT(*) AS n_ventas,
               ROUND(SUM(cantidad), 4) AS total_cantidad,
               ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS total_soles,
               ROUND(AVG(precio_unitario), 4) AS precio_promedio,
               MAX(fecha_orig) AS ultima_venta,
               datetime('now')
        FROM ventas
        GROUP BY id_articulo
    "#).execute(pool).await?;
    Ok(n.rows_affected() as usize)
}

async fn refresh_dim_documento(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_documento").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO dim_documento (folio_unico, tpo_doc, serie_doc, nro_doc, fecha_orig, id_cliente,
                                   id_vendedor, n_lineas, total_cantidad, total_soles, moneda, actualizado_en)
        SELECT folio_unico, tpo_doc, serie_doc, nro_doc, fecha_orig, id_cliente,
               id_vendedor, COUNT(*) AS n_lineas,
               ROUND(SUM(cantidad), 4) AS total_cantidad,
               ROUND(SUM(soles), 2) AS total_soles,
               MAX(moneda) AS moneda,
               datetime('now')
        FROM ventas
        GROUP BY folio_unico
    "#).execute(pool).await?;
    Ok(n.rows_affected() as usize)
}

async fn refresh_dim_linea(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_linea").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO dim_linea (id_linea, nom_linea, id_grupo, nom_grupo,
                               n_ventas, total_soles, n_clientes, actualizado_en)
        SELECT id_linea,
               (SELECT nom_linea FROM ventas v2
                WHERE v2.id_linea = v.id_linea AND v2.nom_linea != ''
                ORDER BY v2.fecha_orig DESC, v2.id DESC LIMIT 1) AS nom_linea,
               (SELECT id_grupo FROM ventas v2
                WHERE v2.id_linea = v.id_linea AND v2.id_grupo != ''
                ORDER BY v2.fecha_orig DESC, v2.id DESC LIMIT 1) AS id_grupo,
               (SELECT nom_grupo FROM ventas v2
                WHERE v2.id_linea = v.id_linea AND v2.nom_grupo != ''
                ORDER BY v2.fecha_orig DESC, v2.id DESC LIMIT 1) AS nom_grupo,
               COUNT(*) AS n_ventas,
               ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS total_soles,
               COUNT(DISTINCT id_cliente) AS n_clientes,
               datetime('now')
        FROM ventas v
        GROUP BY id_linea
    "#).execute(pool).await?;
    Ok(n.rows_affected() as usize)
}

async fn refresh_fact_venta_mes(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM fact_venta_mes").execute(pool).await?;
    let n = sqlx::query(r#"
        INSERT INTO fact_venta_mes (id_linea, id_articulo, mes_ref, cantidad, soles, devuelto, soles_netos)
        SELECT
            id_linea, id_articulo, mes_ref,
            ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END), 4) AS cantidad,
            ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS soles,
            ROUND(SUM(CASE WHEN tipo_operacion = 'devolucion' THEN ABS(cantidad) ELSE 0 END), 4) AS devuelto,
            ROUND(SUM(soles), 2) AS soles_netos
        FROM ventas
        WHERE id_articulo != '' AND id_linea != ''
        GROUP BY id_linea, id_articulo, mes_ref
    "#).execute(pool).await?;
    Ok(n.rows_affected() as usize)
}

// ─── Utilidades ──────────────────────────────────────────────────────────────

/// Analiza la BD y ejecuta OPTIMIZE + ANALYZE para mejorar planes de consulta.
/// Se recomienda correr despues de cargas masivas (>100k filas).
pub async fn analyze_db(pool: &SqlitePool) -> Result<()> {
    sqlx::query("PRAGMA optimize").execute(pool).await?;
    sqlx::query("ANALYZE").execute(pool).await?;
    info!("Database analyzed and optimized");
    Ok(())
}

// ─── Funciones para Tauri UI ─────────────────────────────────────────────────

/// Genera reporte XLSX consolidado con totales por mes para análisis/auditoría.
pub async fn export_monitor_xlsx(pool: &SqlitePool) -> Result<String> {
    use rust_xlsxwriter::{Workbook, Format};

    let rows = monthly_monitor(pool).await?;

    let mut wb = Workbook::new();

    // Formatos
    let header_fmt = Format::new().set_bold().set_background_color("#4472C4").set_font_color("#FFFFFF");
    let money_fmt = Format::new().set_num_format("#,##0.00");
    let int_fmt = Format::new().set_num_format("#,##0");
    let date_fmt = Format::new().set_num_format("yyyy-mm-dd");

    // Hoja 1: Resumen mensual
    let ws = wb.add_worksheet().set_name("Resumen Mensual")?;
    let headers = ["Mes", "Filas", "Total S/", "Total US$", "Clientes", "SKU", "Fecha Inicio", "Fecha Fin"];
    for (i, h) in headers.iter().enumerate() {
        ws.write_string(0, i as u16, h.to_string())?;
    }
    for (i, _) in headers.iter().enumerate() {
        ws.set_column_width(i as u16, 14.0)?;
    }

    // Aplicar formato header
    for col in 0..headers.len() {
        ws.write_string_with_format(0, col as u16, headers[col].to_string(), &header_fmt)?;
    }

    for (idx, row) in rows.iter().enumerate() {
        let r = (idx + 1) as u32;
        ws.write_string(r, 0, row.mes_ref.clone())?;
        ws.write_number_with_format(r, 1, row.filas as f64, &int_fmt)?;
        ws.write_number_with_format(r, 2, row.total_soles, &money_fmt)?;
        ws.write_number_with_format(r, 3, row.total_dolares, &money_fmt)?;
        ws.write_number_with_format(r, 4, row.clientes as f64, &int_fmt)?;
        ws.write_number_with_format(r, 5, row.skus as f64, &int_fmt)?;
        ws.write_string_with_format(r, 6, row.fecha_inicio.clone(), &date_fmt)?;
        ws.write_string_with_format(r, 7, row.fecha_fin.clone(), &date_fmt)?;
    }

    // Hoja 2: Totales generales
    let ws2 = wb.add_worksheet().set_name("Totales")?;
    let total_filas: i64 = rows.iter().map(|r| r.filas).sum();
    let total_soles: f64 = rows.iter().map(|r| r.total_soles).sum();
    let total_dolares: f64 = rows.iter().map(|r| r.total_dolares).sum();
    let total_meses = rows.len();

    ws2.write_string_with_format(0, 0, "Concepto".to_string(), &header_fmt)?;
    ws2.write_string_with_format(0, 1, "Valor".to_string(), &header_fmt)?;
    ws2.write_string_with_format(1, 0, "Total meses".to_string(), &int_fmt)?;
    ws2.write_number(1, 1, total_meses as f64)?;
    ws2.write_string_with_format(2, 0, "Total filas".to_string(), &int_fmt)?;
    ws2.write_number_with_format(2, 1, total_filas as f64, &int_fmt)?;
    ws2.write_string_with_format(3, 0, "Total S/".to_string(), &money_fmt)?;
    ws2.write_number_with_format(3, 1, total_soles, &money_fmt)?;
    ws2.write_string_with_format(4, 0, "Total US$".to_string(), &money_fmt)?;
    ws2.write_number_with_format(4, 1, total_dolares, &money_fmt)?;
    ws2.set_column_width(0, 20.0)?;
    ws2.set_column_width(1, 18.0)?;

    // Guardar
    let export_dir = crate::config::export_dir();
    std::fs::create_dir_all(&export_dir)?;
    let filename = format!("reporte_ventas_{}.xlsx", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
    let path = export_dir.join(&filename);
    wb.save(&path).map_err(|e| anyhow::anyhow!("Error guardando XLSX: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

/// Datos del monitor mensual para el reporte.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MonthlyMonitorRow {
    pub mes_ref: String,
    pub filas: i64,
    pub total_soles: f64,
    pub total_dolares: f64,
    pub clientes: i64,
    pub skus: i64,
    pub fecha_inicio: String,
    pub fecha_fin: String,
}

/// Retorna datos del monitor mensual (totales por mes).
pub async fn monthly_monitor(pool: &SqlitePool) -> Result<Vec<MonthlyMonitorRow>> {
    let rows = sqlx::query_as::<_, (String, i64, f64, f64, i64, i64, String, String)>(r#"
        SELECT
            mes_ref,
            COUNT(*) as filas,
            ROUND(SUM(soles), 2) as total_soles,
            ROUND(COALESCE(SUM(dolares), 0), 2) as total_dolares,
            COUNT(DISTINCT id_cliente) as clientes,
            COUNT(DISTINCT id_articulo) as skus,
            MIN(fecha_orig) as fecha_inicio,
            MAX(fecha_orig) as fecha_fin
        FROM ventas
        WHERE mes_ref IS NOT NULL AND mes_ref != ''
        GROUP BY mes_ref
        ORDER BY mes_ref
    "#).fetch_all(pool).await?;

    Ok(rows.into_iter().map(|r| MonthlyMonitorRow {
        mes_ref: r.0, filas: r.1, total_soles: r.2, total_dolares: r.3,
        clientes: r.4, skus: r.5, fecha_inicio: r.6, fecha_fin: r.7,
    }).collect())
}

/// Exporta NC/NDB a CSV para auditoría.
pub async fn export_nc_ndb(pool: &SqlitePool, desde: &str, hasta: &str) -> Result<Vec<String>> {
    let export_dir = crate::config::export_dir();
    std::fs::create_dir_all(&export_dir)?;

    let rows = sqlx::query_as::<_, (String, String, String, String, String, f64, f64, String)>(r#"
        SELECT fecha_orig, tpo_doc, serie_doc, nro_doc, id_cliente,
               cantidad, soles, tipo_operacion
        FROM ventas
        WHERE tipo_operacion IN ('devolucion', 'ajuste_valor')
          AND fecha_orig >= ? AND fecha_orig <= ?
        ORDER BY fecha_orig
    "#).bind(desde).bind(hasta).fetch_all(pool).await?;

    let filename = format!("nc_ndb_{}_{}.csv", desde, hasta);
    let path = export_dir.join(&filename);
    let mut wtr = csv::Writer::from_path(&path)?;
    wtr.write_record(["fecha_orig", "tpo_doc", "serie_doc", "nro_doc", "id_cliente", "cantidad", "soles", "tipo_operacion"])?;
    for r in rows {
        wtr.write_record([&r.0, &r.1, &r.2, &r.3, &r.4, &r.5.to_string(), &r.6.to_string(), &r.7])?;
    }
    wtr.flush()?;
    Ok(vec![path.to_string_lossy().to_string()])
}

/// Backup de la BD a timestamp.
pub async fn backup_db() -> Result<String> {
    let db_path = crate::config::db_path();
    let backup_dir = crate::config::data_dir().join("backups");
    std::fs::create_dir_all(&backup_dir)?;
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let backup_path = backup_dir.join(format!("historial_{}.db", timestamp));
    std::fs::copy(&db_path, &backup_path)?;
    info!("Backup creado: {}", backup_path.display());
    // Rotación: conservar solo los N más recientes (config.backup_keep)
    let keep = crate::config::load_config().backup_keep;
    if keep > 0 {
        prune_dir_by_age(&backup_dir, "db", keep as usize)?;
    }
    Ok(backup_path.to_string_lossy().to_string())
}

/// Elimina de `dir` los archivos con extensión `ext` más viejos que los `keep`
/// más recientes (orden por mtime). Devuelve cuántos borró.
pub fn prune_dir_by_age(dir: &std::path::Path, ext: &str, keep: usize) -> Result<usize> {
    if !dir.exists() || keep == 0 {
        return Ok(0);
    }
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> =
        std::fs::read_dir(dir)?
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().map(|x| x == ext).unwrap_or(false) {
                    let t = e.metadata().ok()?.modified().ok()?;
                    Some((t, p))
                } else {
                    None
                }
            })
            .collect();
    if files.len() <= keep {
        return Ok(0);
    }
    files.sort_by(|a, b| b.0.cmp(&a.0)); // más reciente primero
    let mut deleted = 0;
    for (_, p) in files.into_iter().skip(keep) {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string());
        if std::fs::remove_file(&p).is_ok() {
            deleted += 1;
            if let Some(n) = name {
                info!("Poda: {}", n);
            }
        }
    }
    if deleted > 0 {
        info!("Rotación {}: {} eliminados (conservados {})", ext, deleted, keep);
    }
    Ok(deleted)
}

/// Rota los cartuchos exportados: conserva los N (.db) más recientes y sus
/// .manifest.json / .zip hermanos; elimina los más viejos.
pub fn prune_cartuchos(keep: usize) -> Result<usize> {
    let dir = crate::config::export_dir();
    if !dir.exists() || keep == 0 {
        return Ok(0);
    }
    let mut dbs: Vec<(std::time::SystemTime, std::path::PathBuf)> =
        std::fs::read_dir(&dir)?
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().map(|x| x == "db").unwrap_or(false) {
                    let t = e.metadata().ok()?.modified().ok()?;
                    Some((t, p))
                } else {
                    None
                }
            })
            .collect();
    if dbs.len() <= keep {
        return Ok(0);
    }
    dbs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut deleted = 0;
    for (_, db_path) in dbs.into_iter().skip(keep) {
        // Hermanos: mismo stem + ".manifest.json" y mismo stem + ".zip"
        if let Some(stem) = db_path.file_stem().map(|s| s.to_string_lossy().to_string()) {
            let manifest = db_path.with_file_name(format!("{}.manifest.json", stem));
            if manifest.exists() {
                let _ = std::fs::remove_file(&manifest);
            }
            let zip = db_path.with_file_name(format!("{}.zip", stem));
            if zip.exists() {
                let _ = std::fs::remove_file(&zip);
            }
        }
        let name = db_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if std::fs::remove_file(&db_path).is_ok() {
            deleted += 1;
            info!("Rotación cartuchos: {}", name);
        }
    }
    Ok(deleted)
}

/// Comprime el último cartucho (.db + .manifest.json) en un ZIP para transporte.
/// Devuelve (ruta_zip, bytes_original, bytes_zip).
pub fn zip_cartucho() -> Result<(String, u64, u64)> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    let export_dir = crate::config::export_dir();
    if !export_dir.exists() {
        anyhow::bail!("No hay carpeta de exportaciones: {}", export_dir.display());
    }

    // Buscar el .db de cartucho más reciente (excluir *_ejemplo si hay base_canonica)
    let mut dbs: Vec<std::path::PathBuf> = std::fs::read_dir(&export_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "db").unwrap_or(false))
        .collect();
    dbs.sort();
    let db_path = dbs
        .last()
        .ok_or_else(|| anyhow::anyhow!("No hay cartucho .db en {}", export_dir.display()))?;

    let manifest_path = db_path.with_extension("manifest.json");
    let zip_path = db_path.with_extension("zip");

    let db_size = std::fs::metadata(db_path)?.len();
    info!("Comprimiendo cartucho: {} ({:.1} MB)", db_path.display(), db_size as f64 / 1048576.0);

    let file = std::fs::File::create(&zip_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(db_path.file_name().unwrap().to_string_lossy(), opts)?;
    let mut src = std::fs::File::open(db_path)?;
    std::io::copy(&mut src, &mut zip)?;

    if manifest_path.exists() {
        zip.start_file(
            manifest_path.file_name().unwrap().to_string_lossy(),
            opts,
        )?;
        let mut src = std::fs::File::open(&manifest_path)?;
        std::io::copy(&mut src, &mut zip)?;
    }
    zip.finish()?;

    let zip_size = std::fs::metadata(&zip_path)?.len();
    info!(
        "ZIP creado: {} ({:.1} MB, {:.0}% del original)",
        zip_path.display(),
        zip_size as f64 / 1048576.0,
        zip_size as f64 * 100.0 / db_size.max(1) as f64
    );
    // Rotación de cartuchos tras cada ZIP
    let keep = crate::config::load_config().cartucho_keep;
    if keep > 0 {
        let _ = prune_cartuchos(keep as usize);
    }
    Ok((
        zip_path.to_string_lossy().to_string(),
        db_size,
        zip_size,
    ))
}

/// Estado del backup local para el recordatorio de la UI.
/// Deriva de la fecha del archivo más reciente en `backups/`.
/// `reminder_days` viene de config (default 7).
pub fn backup_status(reminder_days: u32) -> serde_json::Value {
    let backup_dir = crate::config::data_dir().join("backups");
    let newest = std::fs::read_dir(&backup_dir)
        .ok()
        .and_then(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().map(|x| x == "db").unwrap_or(false))
                .filter_map(|e| e.metadata().ok()?.modified().ok())
                .max()
        });

    match newest {
        Some(t) => {
            let age_days = t
                .elapsed()
                .map(|d| d.as_secs() / 86400)
                .unwrap_or(0);
            let needs = age_days >= reminder_days as u64;
            serde_json::json!({
                "exists": true,
                "age_days": age_days,
                "reminder_days": reminder_days,
                "needs_backup": needs
            })
        }
        None => serde_json::json!({
            "exists": false,
            "age_days": null,
            "reminder_days": reminder_days,
            "needs_backup": true
        }),
    }
}

/// Exporta la BD completa a una ruta específica.
pub async fn export_full_db(target: &std::path::Path) -> Result<String> {
    let db_path = crate::config::db_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&db_path, target)?;
    info!("BD exportada a: {}", target.display());
    Ok(target.to_string_lossy().to_string())
}

/// Vacuum de la BD.
pub async fn vacuum_db(pool: &SqlitePool) -> Result<()> {
    sqlx::query("VACUUM").execute(pool).await?;
    info!("VACUUM completado");
    Ok(())
}

/// Verifica integridad de datos.
pub async fn verify_integrity(pool: &SqlitePool) -> Result<Vec<String>> {
    let mut issues = Vec::new();

    // Verificar fechas futuras
    let future: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE fecha_orig > date('now')")
        .fetch_one(pool).await.unwrap_or(0);
    if future > 0 {
        issues.push(format!("{} filas con fecha futura", future));
    }

    // Verificar montos negativos inesperados
    let neg: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE soles < 0 AND tipo_operacion = 'venta'")
        .fetch_one(pool).await.unwrap_or(0);
    if neg > 0 {
        issues.push(format!("{} ventas con monto negativo", neg));
    }

    // Verificar IDs vacíos
    let empty_ids: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE id_cliente = '' OR id_articulo = ''")
        .fetch_one(pool).await.unwrap_or(0);
    if empty_ids > 0 {
        issues.push(format!("{} filas con ID vacío", empty_ids));
    }

    Ok(issues)
}

/// Reporte de calidad de datos.
pub async fn data_quality_report(pool: &SqlitePool) -> Result<serde_json::Value> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(pool).await.unwrap_or(0);
    let empty_cliente: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE id_cliente = ''").fetch_one(pool).await.unwrap_or(0);
    let empty_sku: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE id_articulo = ''").fetch_one(pool).await.unwrap_or(0);
    let future_dates: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE fecha_orig > date('now')").fetch_one(pool).await.unwrap_or(0);

    let mut critical = Vec::new();
    let mut warn = Vec::new();
    let mut info_items = Vec::new();

    if empty_cliente > 0 { critical.push(format!("{} filas sin id_cliente", empty_cliente)); }
    if empty_sku > 0 { critical.push(format!("{} filas sin id_articulo", empty_sku)); }
    if future_dates > 0 { warn.push(format!("{} filas con fecha futura", future_dates)); }
    info_items.push(format!("Total filas: {}", total));

    Ok(serde_json::json!({
        "total": total,
        "critical": critical,
        "warn": warn,
        "info": info_items
    }))
}

/// Asegura que las tablas de auditoría existan.
pub async fn ensure_audit_tables(pool: &SqlitePool) -> Result<()> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS sync_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            fecha TEXT NOT NULL,
            filas_subidas INTEGER DEFAULT 0,
            meses_synced INTEGER DEFAULT 0,
            estado TEXT DEFAULT 'ok',
            mensaje TEXT
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS checksum_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            mes_ref TEXT NOT NULL,
            checksum TEXT NOT NULL,
            filas INTEGER NOT NULL,
            calculado_en TEXT NOT NULL
        )
    "#).execute(pool).await?;

    Ok(())
}

/// Obtiene historial de sync.
pub async fn get_sync_history(pool: &SqlitePool, limit: i64) -> Result<Vec<serde_json::Value>> {
    let rows = sqlx::query_as::<_, (String, i64, i64, String, Option<String>)>(r#"
        SELECT fecha, filas_subidas, meses_synced, estado, mensaje
        FROM sync_history ORDER BY id DESC LIMIT ?
    "#).bind(limit).fetch_all(pool).await?;

    Ok(rows.into_iter().map(|r| serde_json::json!({
        "fecha": r.0, "filas_subidas": r.1, "meses_synced": r.2,
        "estado": r.3, "mensaje": r.4
    })).collect())
}

/// Obtiene historial de checksums.
pub async fn get_checksum_history(pool: &SqlitePool) -> Result<Vec<serde_json::Value>> {
    let rows = sqlx::query_as::<_, (String, String, i64, String)>(r#"
        SELECT mes_ref, checksum, filas, calculado_en
        FROM checksum_history ORDER BY id DESC LIMIT 50
    "#).fetch_all(pool).await?;

    Ok(rows.into_iter().map(|r| serde_json::json!({
        "mes_ref": r.0, "checksum": r.1, "filas": r.2, "calculado_en": r.3
    })).collect())
}

/// Métricas locales para el dashboard.
pub async fn local_metrics(pool: &SqlitePool) -> Result<serde_json::Value> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(pool).await.unwrap_or(0);
    let meses: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM ventas").fetch_one(pool).await.unwrap_or(0);
    let clientes: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id_cliente) FROM ventas").fetch_one(pool).await.unwrap_or(0);
    let skus: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id_articulo) FROM ventas").fetch_one(pool).await.unwrap_or(0);
    let total_soles: f64 = sqlx::query_scalar("SELECT ROUND(SUM(soles), 2) FROM ventas").fetch_one(pool).await.unwrap_or(0.0);

    Ok(serde_json::json!({
        "total_records": total,
        "total_months": meses,
        "total_clients": clientes,
        "total_skus": skus,
        "total_sales": total_soles
    }))
}
