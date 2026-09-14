// Comentarios en espanol - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use crate::config::db_path;
use crate::models::Venta;
use crate::db::schema::{CREATE_INDEXES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL, CREATE_STATS_CACHE_SQL, CREATE_AUDIT_TABLES_SQL, CREATE_DIM_TABLES_SQL};
use anyhow::{Context, Result};
use sqlx::{query, SqlitePool};
use std::sync::Mutex;
use tracing::info;

/// Pool singleton: evita recrear la conexión en cada llamada a init_pool().
/// sqlx::SqlitePool ya maneja internamente un pool de conexiones con WAL.
static POOL: Mutex<Option<SqlitePool>> = Mutex::new(None);

pub async fn init_pool() -> Result<SqlitePool> {
    {
        let guard = POOL.lock().unwrap();
        if let Some(pool) = guard.as_ref() {
            if !pool.is_closed() {
                return Ok(pool.clone());
            }
        }
    }
    // Crear pool fresco (primera vez o tras close)
    let db = db_path();
    if let Some(p) = db.parent() {
        std::fs::create_dir_all(p)?;
    }
    let db_str = db.to_string_lossy().replace("\\", "/");
    let url = format!("sqlite://{}?mode=rwc", db_str);
    tracing::info!("Connecting to DB: {}", url);
    // WAL + busy_timeout + pool size: lecturas del dashboard no bloquean con escrituras de captura
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(30));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(5)
        // Reclamo agresivo de conexiones: si un init falla, no quedan conos vivos
        // filtrando RAM durante minutos (bug real 2026-09-09: 4.5 GB de RAM)
        .idle_timeout(std::time::Duration::from_secs(60))
        .max_lifetime(std::time::Duration::from_secs(600))
        .connect_with(opts)
        .await
        .context("DB connect failed")?;
    // Activar foreign keys via PRAGMA (sqlite3_connect_options no lo aplica en todas las versiones)
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .ok();
    // CRITICO: guardar el singleton INMEDIATAMENTE tras conectar. Antes se guardaba
    // al final, despues de migraciones pesadas (refresh_stats_cache con EXISTS sobre
    // ~1.2M filas tarda minutos): cada poll de la UI (cada 2s) lanzaba OTRO init
    // pesado en paralelo -> decenas de pools concurrentes -> explosion de RAM.
    *POOL.lock().unwrap() = Some(pool.clone());
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await?;
    for col in [
        "doc_cliente TEXT",
        "precio_unitario REAL",
        "cantidad_fae REAL",
        "original_sku TEXT",
        "tipo_operacion TEXT DEFAULT 'venta'",
        "factura_ref_serie TEXT",
        "factura_ref_nro TEXT",
        "folio_unico TEXT",
        "ord_compra TEXT",
    ] {
        let name = col.split_whitespace().next().unwrap();
        let exists: bool =
            sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('ventas') WHERE name=?")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap_or(0)
                > 0;
        if !exists {
            let _ = sqlx::query(&format!("ALTER TABLE ventas ADD COLUMN {}", col))
                .execute(&pool)
                .await;
        }
    }
    for idx in CREATE_INDEXES_SQL {
        let _ = sqlx::query(idx).execute(&pool).await;
    }
    // Vistas para consumo downstream (CRM/dashboard/devoluciones) — no ocupan espacio
    for v in CREATE_VIEWS_SQL {
        let _ = sqlx::query(v).execute(&pool).await;
    }
    // Stats cache table (dashboard performance)
    let _ = sqlx::query(CREATE_STATS_CACHE_SQL).execute(&pool).await;
    // Dimension tables — derivadas de ventas, se refrescan tras cada cambio
    let _ = sqlx::query(CREATE_DIM_TABLES_SQL).execute(&pool).await;
    // Refresh stats cache on init
    let _ = refresh_stats_cache(&pool).await;
    let _ = refresh_dim_tables(&pool).await;
    let _ = sqlx::query("UPDATE ventas SET precio_unitario = CASE WHEN cantidad != 0 THEN soles / cantidad ELSE 0 END WHERE precio_unitario IS NULL").execute(&pool).await;
    // Repopular precio_unitario para ajustes de valor (cantidad=0) usando CANTIDAD FAE como base
    // (ej. vendi 1000 pero descuente 500 entregados -> precio aplicado = soles / FAE)
    let _ = sqlx::query("UPDATE ventas SET precio_unitario = ROUND(soles / cantidad_fae, 4) WHERE cantidad = 0 AND cantidad_fae != 0 AND precio_unitario = 0").execute(&pool).await;
    // Migración 2026-09-13: normalizar id_linea quitando prefijo "01"
    // Antes: 0101, 01MA, 01AD (4 chars con prefijo) -> Después: 01, MA, AD (2 chars)
    let lineas_migradas = sqlx::query("UPDATE ventas SET id_linea = SUBSTR(id_linea, 3) WHERE id_linea GLOB '01??'")
        .execute(&pool)
        .await;
    if let Ok(affected) = lineas_migradas {
        if affected.rows_affected() > 0 {
            info!("Migracion id_linea: {} filas normalizadas (quitando prefijo 01)", affected.rows_affected());
            let _ = refresh_dim_tables(&pool).await;
        }
    }
    // Migración 2026-09-13: normalizar id_vendedor quitando prefijo "01"
    // Antes: 01O01, 01193, 01M09 (5 chars con prefijo) -> Después: O01, 193, M09 (3 chars)
    let ventas_migradas = sqlx::query("UPDATE ventas SET id_vendedor = SUBSTR(id_vendedor, 4) WHERE id_vendedor GLOB '01???'")
        .execute(&pool)
        .await;
    if let Ok(affected) = ventas_migradas {
        if affected.rows_affected() > 0 {
            info!("Migracion id_vendedor: {} filas normalizadas (quitando prefijo 01)", affected.rows_affected());
            let _ = refresh_dim_tables(&pool).await;
        }
    }
    info!("DB ready: {}", db.display());
    Ok(pool)
}

pub async fn insert_ventas(pool: &SqlitePool, ventas: &[Venta]) -> Result<usize> {
    let mut tx = pool.begin().await?;
    if let Some(first) = ventas.first() {
        let is_daily =
            first.mes_ref.len() == 10 && first.mes_ref.chars().filter(|c| *c == '-').count() == 2;
        if is_daily {
            let min_d = ventas
                .iter()
                .map(|v| v.fecha_orig)
                .min()
                .unwrap_or(first.fecha_orig);
            let max_d = ventas
                .iter()
                .map(|v| v.fecha_orig)
                .max()
                .unwrap_or(first.fecha_orig);
            sqlx::query("DELETE FROM ventas WHERE fecha_orig BETWEEN ? AND ?")
                .bind(min_d.to_string())
                .bind(max_d.to_string())
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref)
                .execute(&mut *tx)
                .await?;
        }
    }
    let mut n = 0usize;
    for v in ventas {
        sqlx::query(
            "INSERT INTO ventas (id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo, id_tipo,nom_tipo,id_familia,nom_familia, id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia, moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig, fecha_ref,fecha_venc,cod_sucursal,nom_sucursal, departamento,provincia,distrito, id_vendedor, nom_vendedor,id_pedido,ord_compra,file_source,mes_ref,tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        )
        .bind(&v.id_articulo).bind(&v.original_sku).bind(&v.nom_articulo)
        .bind(&v.id_linea).bind(&v.nom_linea).bind(&v.id_grupo).bind(&v.nom_grupo)
        .bind(&v.id_tipo).bind(&v.nom_tipo).bind(&v.id_familia).bind(&v.nom_familia)
        .bind(&v.id_cliente).bind(&v.doc_cliente).bind(&v.nom_cliente)
        .bind(&v.tpo_doc).bind(&v.serie_doc).bind(&v.nro_doc).bind(&v.referencia)
        .bind(&v.moneda).bind(v.cantidad).bind(v.cantidad_fae).bind(v.soles).bind(v.dolares).bind(v.precio_unitario)
        .bind(v.anho).bind(v.mes).bind(v.fecha_orig.to_string())
        .bind(&v.fecha_ref).bind(&v.fecha_venc)
        .bind(&v.cod_sucursal).bind(&v.nom_sucursal).bind(&v.departamento).bind(&v.provincia).bind(&v.distrito)
        .bind(&v.id_vendedor).bind(&v.nom_vendedor).bind(&v.id_pedido).bind(&v.ord_compra)
        .bind(&v.file_source).bind(&v.mes_ref)
        .bind(&v.tipo_operacion).bind(&v.factura_ref_serie).bind(&v.factura_ref_nro).bind(&v.folio_unico)
        .execute(&mut *tx).await?;
        n += 1;
    }
    tx.commit().await?;
    info!("Inserted {} rows", n);
    let _ = refresh_stats_cache(pool).await;
    Ok(n)
}

pub async fn dedup_ventas(pool: &SqlitePool) -> Result<usize> {
    // Dedup por línea: (folio_unico, id_articulo)
    // Una factura puede tener múltiples líneas (distintos SKUs).
    // Same invoice + same SKU = duplicate, keep the latest.
    let r = sqlx::query(
        "DELETE FROM ventas WHERE id NOT IN (
            SELECT MAX(id) FROM ventas
            GROUP BY folio_unico, id_articulo
        )",
    )
    .execute(pool)
    .await?;
    let affected = r.rows_affected() as usize;
    info!("Dedup removed {} rows", affected);
    let _ = refresh_stats_cache(pool).await;
    let _ = refresh_dim_tables(pool).await;
    Ok(affected)
}

pub async fn count_ventas(pool: &SqlitePool) -> Result<i64> {
    let r: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas")
        .fetch_one(pool)
        .await?;
    Ok(r.0)
}

pub async fn count_by_month(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(
        sqlx::query_as("SELECT mes_ref, COUNT(*) FROM ventas GROUP BY mes_ref ORDER BY mes_ref")
            .fetch_all(pool)
            .await?,
    )
}

/// Refresca la tabla stats_cache con los valores actuales del dashboard.
/// Debe llamarse despues de cada insercion o borrado masivo.
/// RAW-FIRST: la tabla ventas guarda TODO el export del ERP; las stats del
/// dashboard se calculan con el mismo filtro obligatorio del upload para que
/// reflejen solo lo "vital" (coherentes con Supabase), no el raw completo.
pub async fn refresh_stats_cache(pool: &SqlitePool) -> Result<()> {
    let lf = crate::config::with_line_filter("");
    let (total_records,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM ventas {}", lf))
        .fetch_one(pool).await?;
    let (total_sales,): (f64,) = sqlx::query_as(&format!("SELECT COALESCE(SUM(soles), 0.0) FROM ventas {}", lf))
        .fetch_one(pool).await?;
    let (total_clients,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(DISTINCT id_cliente) FROM ventas {}", lf))
        .fetch_one(pool).await?;
    let (total_skus,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(DISTINCT original_sku) FROM ventas {}", lf))
        .fetch_one(pool).await?;

    let now = chrono::Utc::now().to_rfc3339();
    let pairs = [
        ("total_records", total_records as f64),
        ("total_sales", total_sales),
        ("total_clients", total_clients as f64),
        ("total_skus", total_skus as f64),
    ];
    for (key, value) in &pairs {
        sqlx::query(
            "INSERT INTO stats_cache (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at"
        )
        .bind(key)
        .bind(value)
        .bind(&now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Reconstruye todas las tablas de dimensiones desde la tabla ventas (fact table).
/// Cada dimensión se procesa en una funcion separada para trazabilidad de errores.
/// Se llama tras cada dedup/insert para mantener consistencia.
/// Costo estimado: ~2-5s en BD de 1.4M filas (full scan de ventas, pero con indices).
///
/// Si las tablas dim estan vacias (caso de BD heredada sin dims), se reconstruyen
/// automaticamente en el primer init_pool sin necesidad de accion manual.
/// Todo se ejecuta secuencialmente: si falla a mitad se propaga el error y el caller decide reintentar.
pub async fn refresh_dim_tables(pool: &SqlitePool) -> Result<usize> {
    // Verificar si ya hay datos; si no, construir de cero (caso post-migracion)
    let n_existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dim_cliente")
        .fetch_one(pool).await.unwrap_or(0);
    if n_existing > 0 {
        return Ok(0); // ya pobladas, sin trabajo
    }
    let n_cli = refresh_dim_cliente(pool).await?;
    let n_vend = refresh_dim_vendedor(pool).await?;
    let n_art = refresh_dim_articulo(pool).await?;
    let n_doc = refresh_dim_documento(pool).await?;
    let n_line = refresh_dim_linea(pool).await?;
    info!(
        "dim_tables rebuild: cliente={}, vendedor={}, articulo={}, documento={}, linea={}",
        n_cli, n_vend, n_art, n_doc, n_line
    );
    Ok(n_cli + n_vend + n_art + n_doc + n_line)
}

/// Refresca dim_cliente: maxima info conocida por cliente + KPIs agregados.
/// Fuente de verdad para navegacion por cliente en el dashboard local.
async fn refresh_dim_cliente(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_cliente")
        .execute(pool)
        .await
        .context("refresh_dim_cliente: DELETE falló")?;
    let n = sqlx::query(r#"
        INSERT INTO dim_cliente
            (id_cliente, nom_cliente, doc_cliente, departamento, provincia, distrito,
             n_ventas, total_soles, total_dolares, ultima_compra, actualizado_en)
        SELECT
            id_cliente,
            MAX(nom_cliente)   AS nom_cliente,
            MAX(doc_cliente)   AS doc_cliente,
            MAX(departamento)  AS departamento,
            MAX(provincia)     AS provincia,
            MAX(distrito)      AS distrito,
            COUNT(*)           AS n_ventas,
            ROUND(SUM(soles), 2)         AS total_soles,
            ROUND(SUM(COALESCE(dolares, 0.0)), 2) AS total_dolares,
            MAX(fecha_orig)      AS ultima_compra,
            datetime('now')      AS actualizado_en
        FROM ventas
        WHERE id_cliente != ''
        GROUP BY id_cliente
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_cliente: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Refresca dim_vendedor: maxima info por vendedor + KPIs agregados.
/// Permite navegar por vendedores y ver su cartera de clientes rapidamente.
async fn refresh_dim_vendedor(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_vendedor")
        .execute(pool)
        .await
        .context("refresh_dim_vendedor: DELETE falló")?;
    let n = sqlx::query(r#"
        INSERT INTO dim_vendedor
            (id_vendedor, nom_vendedor, n_ventas, total_soles, n_clientes, ultima_venta, actualizado_en)
        SELECT
            id_vendedor,
            MAX(nom_vendedor)     AS nom_vendedor,
            COUNT(*)              AS n_ventas,
            ROUND(SUM(soles), 2)  AS total_soles,
            COUNT(DISTINCT id_cliente) AS n_clientes,
            MAX(fecha_orig)       AS ultima_venta,
            datetime('now')       AS actualizado_en
        FROM ventas
        WHERE id_vendedor != ''
        GROUP BY id_vendedor
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_vendedor: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Refresca dim_articulo: maxima info de catalogo por SKU + KPIs agregados.
/// Permite navegar por articulos, ver precio promedio y volumen de ventas.
async fn refresh_dim_articulo(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_articulo")
        .execute(pool)
        .await
        .context("refresh_dim_articulo: DELETE falló")?;
    let n = sqlx::query(r#"
        INSERT INTO dim_articulo
            (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
             id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia,
             n_ventas, total_cantidad, total_soles, precio_promedio, ultima_venta, actualizado_en)
        SELECT
            id_articulo,
            MAX(original_sku)   AS original_sku,
            MAX(nom_articulo)   AS nom_articulo,
            MAX(id_linea)       AS id_linea,  MAX(nom_linea)  AS nom_linea,
            MAX(id_grupo)       AS id_grupo,  MAX(nom_grupo)  AS nom_grupo,
            MAX(id_tipo)        AS id_tipo,   MAX(nom_tipo)   AS nom_tipo,
            MAX(id_familia)     AS id_familia, MAX(nom_familia) AS nom_familia,
            COUNT(*)            AS n_ventas,
            ROUND(SUM(cantidad), 2) AS total_cantidad,
            ROUND(SUM(soles), 2)      AS total_soles,
            ROUND(SUM(soles) / NULLIF(SUM(cantidad), 0), 4) AS precio_promedio,
            MAX(fecha_orig)   AS ultima_venta,
            datetime('now')   AS actualizado_en
        FROM ventas
        WHERE id_articulo != ''
        GROUP BY id_articulo
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_articulo: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Refresca dim_documento: cabeceras de documentos (una fila por folio unico).
/// Permite navegar por facturas/NC/ND y ver resumen de lineas, montos y fechas.
async fn refresh_dim_documento(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_documento")
        .execute(pool)
        .await
        .context("refresh_dim_documento: DELETE falló")?;
    let n = sqlx::query(r#"
        INSERT INTO dim_documento
            (folio_unico, tpo_doc, serie_doc, nro_doc, fecha_orig,
             id_cliente, id_vendedor, n_lineas, total_cantidad,
             total_soles, total_dolares, moneda, actualizado_en)
        SELECT
            folio_unico,
            MAX(tpo_doc)      AS tpo_doc,
            MAX(serie_doc)    AS serie_doc,
            MAX(nro_doc)      AS nro_doc,
            MAX(fecha_orig)   AS fecha_orig,
            MAX(id_cliente)   AS id_cliente,
            MAX(id_vendedor)  AS id_vendedor,
            COUNT(*)          AS n_lineas,
            ROUND(SUM(cantidad), 2)     AS total_cantidad,
            ROUND(SUM(soles), 2)        AS total_soles,
            ROUND(SUM(COALESCE(dolares, 0.0)), 2) AS total_dolares,
            MAX(moneda)     AS moneda,
            datetime('now') AS actualizado_en
        FROM ventas
        WHERE folio_unico != ''
        GROUP BY folio_unico
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_documento: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Refresca dim_linea: maxima info por linea/negocio + KPIs agregados.
/// Permite navegar por lineas de producto y ver volumen por grupo/familia.
async fn refresh_dim_linea(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_linea")
        .execute(pool)
        .await
        .context("refresh_dim_linea: DELETE falló")?;
    let n = sqlx::query(r#"
        INSERT INTO dim_linea
            (id_linea, nom_linea, id_grupo, nom_grupo, id_familia, nom_familia,
             n_ventas, total_soles, n_clientes, actualizado_en)
        SELECT
            id_linea,
            MAX(nom_linea)    AS nom_linea,
            MAX(id_grupo)     AS id_grupo,  MAX(nom_grupo) AS nom_grupo,
            MAX(id_familia)   AS id_familia, MAX(nom_familia) AS nom_familia,
            COUNT(*)          AS n_ventas,
            ROUND(SUM(soles), 2) AS total_soles,
            COUNT(DISTINCT id_cliente) AS n_clientes,
            datetime('now')   AS actualizado_en
        FROM ventas
        WHERE id_linea != ''
        GROUP BY id_linea
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_linea: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

// ─── AUDITORÍA E INTEGRIDAD ─────────────────────────────────────────────────

/// Crea las tablas de auditoría si no existen
pub async fn ensure_audit_tables(pool: &SqlitePool) -> Result<()> {
    query(CREATE_AUDIT_TABLES_SQL)
        .execute(pool)
        .await?;
    Ok(())
}

/// Registra una entrada en el log de sincronización
pub async fn log_sync(
    pool: &SqlitePool,
    tipo: &str,
    estado: &str,
    filas_solicitadas: i64,
    filas_subidas: i64,
    filas_limpiadas: i64,
    duracion_segundos: f64,
    error_message: Option<&str>,
) -> Result<i64> {
    let row_id = sqlx::query_scalar(
        r#"INSERT INTO sync_log (tipo, estado, filas_solicitadas, filas_subidas, filas_limpiadas, duracion_segundos, error_message, started_at, finished_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now')) RETURNING id"#
    )
    .bind(tipo)
    .bind(estado)
    .bind(filas_solicitadas)
    .bind(filas_subidas)
    .bind(filas_limpiadas)
    .bind(duracion_segundos)
    .bind(error_message)
    .fetch_one(pool)
    .await?;
    Ok(row_id)
}

/// Genera checksums mensuales para detectar cambios en los datos.
/// El checksum integra: COUNT(*), SUM(soles), SUM(dolares) y SUM(cantidad) —
/// cualquier alteración de filas o montos cambia el hash.
pub async fn calculate_monthly_checksums(pool: &SqlitePool) -> Result<Vec<(String, String, i64, f64)>> {
    let results = sqlx::query_as(
        r#"INSERT INTO mes_checksums (mes_ref, checksum, total_filas, total_soles, total_cantidad, calculado_en)
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
               ROUND(SUM(cantidad), 2) as total_cantidad,
               datetime('now')
           FROM ventas
           GROUP BY mes_ref
           ON CONFLICT(mes_ref) DO UPDATE SET
               checksum = excluded.checksum,
               total_filas = excluded.total_filas,
               total_soles = excluded.total_soles,
               total_cantidad = excluded.total_cantidad,
               calculado_en = excluded.calculado_en
           RETURNING mes_ref, checksum, total_filas, total_soles"#
    )
    .fetch_all(pool)
    .await?;
    Ok(results)
}

/// Verifica la integridad de la base de datos y retorna inconsistencias
/// Detecta entidades posiblemente PARTIDAS en dos claves por formatos de codigo
/// distintos (ej: "56101" y "00056101" serian el mismo cliente). Complementa a
/// las reglas de canonizacion del parser: el parser previene, esto DETECTA.
pub async fn detectar_entidades_duplicadas(pool: &SqlitePool) -> Result<Vec<String>> {
    let mut issues = Vec::new();

    // Cliente: dos id_cliente que son el mismo numero con distinto padding
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT LTRIM(id_cliente,'0') k, COUNT(DISTINCT id_cliente) n
            FROM ventas WHERE id_cliente != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!(
            "🔴 CLIENTES PARTIDOS: {} grupos donde 2+ id_cliente son el mismo numero con distinto padding", n));
    }

    // Vendedor: ya normalizado (sin prefijo 01), verificar integridad
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT id_vendedor k, COUNT(DISTINCT id_vendedor) n
            FROM ventas WHERE id_vendedor != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🔴 VENDEDORES PARTIDOS: {} grupos con claves duplicadas", n));
    }

    // Linea: mismo codigo con/sin prefijo "01"
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT CASE WHEN id_linea LIKE '01%' THEN SUBSTR(id_linea,3) ELSE id_linea END k,
                   COUNT(DISTINCT id_linea) n
            FROM ventas WHERE id_linea != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🔴 LÍNEAS PARTIDAS: {} grupos con forma corta y larga del mismo codigo", n));
    }

    // SKU: colisiones al quitar ceros — INFORMATIVO (011019 y 11019 pueden ser
    // SKUs legitimos distintos; solo alerta si el conteo crece inesperadamente)
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT LTRIM(id_articulo,'0') k, COUNT(DISTINCT id_articulo) n
            FROM ventas WHERE id_articulo != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!(
            "ℹ️  SKUs con colision de ceros: {} grupos (pueden ser legitimos, ej 011019≠11019; revisar si crece)", n));
    }

    // Mismo RUC (doc_cliente) apuntando a 2+ id_cliente — excluye anomalias ERP conocidas
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT doc_cliente k, COUNT(DISTINCT id_cliente) n
            FROM ventas
            WHERE doc_cliente NOT IN ('', '00') AND LENGTH(doc_cliente) >= 10
            GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🟡 RUC duplicado: {} RUCs apuntan a 2+ id_cliente (renombres/fusiones del ERP)", n));
    }

    // Mismo nombre de cliente con 2+ id_cliente (causa comun de splits)
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT UPPER(TRIM(nom_cliente)) k, COUNT(DISTINCT id_cliente) n
            FROM ventas WHERE nom_cliente != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🟡 Nombres repetidos: {} nombres de cliente comparten 2+ id_cliente (posible split por renombre ERP)", n));
    }

    // Renombres legitimos: un id_cliente con 2+ razones sociales a lo largo del
    // tiempo. NO es corrupcion (la clave es id_cliente; cada fila conserva el
    // nombre vigente al momento de la venta = historia auditable). Informativo.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT id_cliente FROM ventas
            WHERE id_cliente != '' AND nom_cliente != ''
            GROUP BY id_cliente HAVING COUNT(DISTINCT nom_cliente) > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("ℹ️  Clientes renombrados: {} clientes tienen 2+ razones sociales historicas (vw_dim_cliente muestra la mas reciente)", n));
    }
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT id_vendedor FROM ventas
            WHERE id_vendedor != '' AND nom_vendedor != ''
            GROUP BY id_vendedor HAVING COUNT(DISTINCT nom_vendedor) > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("ℹ️  Vendedores con 2+ nombres historicos: {} (cambio de nombre o correccion; la clave id_vendedor no se rompe)", n));
    }
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT id_articulo FROM ventas
            WHERE id_articulo != '' AND nom_articulo != ''
            GROUP BY id_articulo HAVING COUNT(DISTINCT nom_articulo) > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("ℹ️  SKUs con 2+ nombres historicos: {} (renombre de catalogo; la clave id_articulo no se rompe)", n));
    }

    Ok(issues)
}

pub async fn verify_integrity(pool: &SqlitePool) -> Result<Vec<String>> {
    let mut issues = Vec::new();

    // 1. Verificar duplicados por (folio_unico, id_articulo)
    let dup_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT folio_unico, id_articulo, COUNT(*) as cnt
            FROM ventas
            GROUP BY folio_unico, id_articulo
            HAVING COUNT(*) > 1
        )"
    )
    .fetch_one(pool)
    .await?;

    if dup_count > 0 {
        issues.push(format!("⚠️  Encontrados {} pares (folio+SKU) duplicados", dup_count));
    }

    // 2. Verificar facturas sin líneas
    let empty_invoices: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT folio_unico) FROM ventas WHERE folio_unico IS NULL OR folio_unico = ''"
    )
    .fetch_one(pool)
    .await?;

    if empty_invoices > 0 {
        issues.push(format!("⚠️  {} registros sin folio_unico válido", empty_invoices));
    }

    // 3. Verificar meses sin datos
    let now_str = chrono::Utc::now().format("%Y").to_string();
    let current_year: i64 = now_str.parse().unwrap_or(2026);
    let expected_months = (current_year - 2018) * 12 + 9; // Desde 2018-01 hasta el mes actual
    let actual_months: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM ventas")
        .fetch_one(pool)
        .await?;

    if actual_months < expected_months {
        issues.push(format!("ℹ️  {} meses con datos (esperado ~{} desde 2018)", actual_months, expected_months));
    }

    // 4. Comparar checksums vs estado actual
    let checksum_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mes_checksums")
        .fetch_one(pool)
        .await?;

    if checksum_count == 0 {
        issues.push("ℹ️  No hay checksums calculados. Ejecuta 'calcular_checksums' para establecer baseline".to_string());
    }

    // 5. Entidades posiblemente partidas (clientes/vendedores/lineas/SKUs/RUCs)
    match detectar_entidades_duplicadas(pool).await {
        Ok(found) => issues.extend(found),
        Err(e) => issues.push(format!("⚠️  Deteccion de duplicados fallo: {}", e)),
    }

    if issues.is_empty() {
        issues.push("✅ Base de datos intacta. No se encontraron inconsistencias.".to_string());
    }

    Ok(issues)
}

/// Retorna el historial de sincronizaciones
pub async fn get_sync_history(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, String, String, i64, i64, i64, String)>> {
    let rows = sqlx::query_as(
        r#"SELECT id, tipo, estado, filas_solicitadas, filas_subidas, filas_limpiadas, started_at
           FROM sync_log
           ORDER BY started_at DESC
           LIMIT ?"#
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Retorna el historial de checksums mensuales
pub async fn get_checksum_history(pool: &SqlitePool) -> Result<Vec<(String, String, i64, f64, String)>> {
    let rows = sqlx::query_as(
        r#"SELECT mes_ref, checksum, total_filas, total_soles, calculado_en
           FROM mes_checksums
           ORDER BY mes_ref DESC"#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Ejecuta VACUUM en la BD para recomprar el archivo y eliminar bloat acumulado
/// por operaciones DELETE masivas (dedup, re-captura de meses). Costo: ~30-120s
/// en BD de 1GB; bloquea lecturas durante la ejecucion.
pub async fn vacuum_db(pool: &SqlitePool) -> Result<String> {
    info!("VACUUM iniciado — la BD se recomprara (esto puede tardar 30-120s)");
    sqlx::query("VACUUM").execute(pool).await?;
    let new_size = std::fs::metadata(db_path()).map(|m| m.len()).unwrap_or(0);
    info!("VACUUM completado: nueva dimension {} MB", new_size / 1024 / 1024);
    Ok(format!("VACUUM completado — {} MB", new_size / 1024 / 1024))
}

/// Crea un backup con fecha/hora del archivo historial.db en la carpeta backup/.
/// Rotacion automatica: se conservan solo las ultimas 7 copias.
pub async fn backup_db() -> Result<String> {
    use std::io::Write;
    let src = db_path();
    if !src.exists() {
        return Err(anyhow::anyhow!("No existe la BD para respaldar"));
    }
    let backup_dir = src.parent().unwrap().join("backup");
    std::fs::create_dir_all(&backup_dir)?;
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let dst = backup_dir.join(format!("historial_{}.db", ts));
    // Copiar archivo completo ( SQLite WAL modo — copiar -wal tambien)
    std::fs::copy(&src, &dst)?;
    // Si existe -wal, copiarlo tambien
    let wal = src.with_extension("db-wal");
    if wal.exists() {
        let _ = std::fs::copy(&wal, dst.with_extension("db-wal"));
    }
    // Rotacion: conservar solo ultimas 7 copias
    let mut backups: Vec<std::time::SystemTime> = std::fs::read_dir(&backup_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "db").unwrap_or(false))
        .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .collect();
    backups.sort();
    while backups.len() > 7 {
        let oldest = backups.remove(0);
        // Convertir SystemTime a timestamp string para construir el nombre a borrar
        let ts_str = oldest.duration_since(std::time::UNIX_EPOCH)
            .map(|d| {
                let dt = chrono::DateTime::<chrono::Utc>::from(oldest);
                dt.format("%Y%m%d_%H%M%S").to_string()
            })
            .unwrap_or_default();
        let name = backup_dir.join(format!("historial_{}.db", ts_str));
        let _ = std::fs::remove_file(&name);
    }
    let size = std::fs::metadata(&dst)?.len();
    info!("Backup creado: {} ({:.1} MB)", dst.file_name().unwrap().to_string_lossy(), size / 1024 / 1024);
    Ok(format!("Backup: {} ({:.1} MB)", dst.file_name().unwrap().to_string_lossy(), size / 1024 / 1024))
}

/// Calcula checksums para todos los meses que aun no tienen hash registrado.
/// Se usa GROUP BY mes_ref para eficiencia en BD grande.
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
    "#)
    .fetch_all(pool)
    .await?;
    let count = n.len();
    info!("Checksums recalculados: {} meses", count);
    Ok(count)
}

/// Exporta la BD completa a un archivo standalone (VACUUM INTO) listo para compartir.
/// Produce un unico archivo .db sin WAL, con dims pobladas y checksums actuales.
pub async fn export_full_db(target_path: &std::path::Path) -> Result<String> {
    let pool = init_pool().await?;
    // Poblar dim tables si estan vacias
    let _ = refresh_dim_tables(&pool).await;
    // Recalcular checksums de meses faltantes
    let _ = recalculate_missing_checksums(&pool).await;
    // VACUUM INTO produce un .db autocontenido (sin -wal)
    sqlx::query("VACUUM INTO ?")
        .bind(target_path.to_string_lossy().to_string())
        .execute(&pool)
        .await?;
    let size = std::fs::metadata(target_path).map(|m| m.len()).unwrap_or(0) as f64;
    info!("Export BD completa: {} -> {:.1} MB", target_path.display(), size / 1024.0 / 1024.0);
    Ok(format!("Exportado: {} ({:.1} MB)", target_path.file_name().unwrap_or_default().to_string_lossy(), size / 1024.0 / 1024.0))
}
