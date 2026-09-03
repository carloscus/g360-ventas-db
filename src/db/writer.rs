// Comentarios en espanol - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use crate::config::db_path;
use crate::models::Venta;
use crate::db::schema::{CREATE_INDEXES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL, CREATE_STATS_CACHE_SQL, CREATE_AUDIT_TABLES_SQL};
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
        .busy_timeout(std::time::Duration::from_secs(30));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(20)
        .connect_with(opts)
        .await
        .context("DB connect failed")?;
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
    // Refresh stats cache on init
    let _ = refresh_stats_cache(&pool).await;
    let _ = sqlx::query("UPDATE ventas SET precio_unitario = CASE WHEN cantidad != 0 THEN soles / cantidad ELSE 0 END WHERE precio_unitario IS NULL").execute(&pool).await;
    // Repopular precio_unitario para ajustes de valor (cantidad=0) usando CANTIDAD FAE como base
    // (ej. vendi 1000 pero descuente 500 entregados -> precio aplicado = soles / FAE)
    let _ = sqlx::query("UPDATE ventas SET precio_unitario = ROUND(soles / cantidad_fae, 4) WHERE cantidad = 0 AND cantidad_fae != 0 AND precio_unitario = 0").execute(&pool).await;
    info!("DB ready: {}", db.display());
    // Almacenar en singleton para reusar en todas las llamadas
    *POOL.lock().unwrap() = Some(pool.clone());
    Ok(pool)
}

pub async fn insert_ventas(pool: &SqlitePool, ventas: &[Venta]) -> Result<usize> {
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
            let _ = sqlx::query("DELETE FROM ventas WHERE fecha_orig BETWEEN ? AND ?")
                .bind(min_d.to_string())
                .bind(max_d.to_string())
                .execute(pool)
                .await;
        } else {
            let _ = sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref)
                .execute(pool)
                .await;
        }
    }
    let mut tx = pool.begin().await?;
    let mut n = 0usize;
    for v in ventas {
        sqlx::query(
            "INSERT INTO ventas (id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo, id_tipo,nom_tipo,id_familia,nom_familia, id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia, moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig, fecha_ref,fecha_venc,cod_sucursal,nom_sucursal, departamento,provincia,distrito, id_vendedor, nom_vendedor,id_pedido,file_source,mes_ref,tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
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
        .bind(&v.id_vendedor).bind(&v.nom_vendedor).bind(&v.id_pedido)
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
    Ok(affected)
}

pub async fn count_ventas(pool: &SqlitePool) -> Result<i64> {
    let r: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas")
        .fetch_one(pool)
        .await?;
    Ok(r.0)
}

pub async fn fetch_all_ventas(pool: &SqlitePool) -> Result<Vec<Venta>, sqlx::Error> {
    use sqlx::Row;
    let rows = query("SELECT id_articulo, original_sku, nom_articulo, id_linea, nom_linea, id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia, id_cliente, doc_cliente, nom_cliente, tpo_doc, serie_doc, nro_doc, referencia, moneda, cantidad, cantidad_fae, soles, dolares, precio_unitario, anho, mes, fecha_orig, fecha_ref, fecha_venc, cod_sucursal, nom_sucursal, departamento, provincia, distrito, id_vendedor, nom_vendedor, id_pedido, file_source, mes_ref, tipo_operacion, factura_ref_serie, factura_ref_nro, folio_unico FROM ventas")
        .fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| Venta {
            id_articulo: r.get(0),
            original_sku: r.get(1),
            nom_articulo: r.get(2),
            id_linea: r.get(3),
            nom_linea: r.get(4),
            id_grupo: r.get(5),
            nom_grupo: r.get(6),
            id_tipo: r.get(7),
            nom_tipo: r.get(8),
            id_familia: r.get(9),
            nom_familia: r.get(10),
            id_cliente: r.get(11),
            doc_cliente: r.get(12),
            nom_cliente: r.get(13),
            tpo_doc: r.get(14),
            serie_doc: r.get(15),
            nro_doc: r.get(16),
            referencia: r.get(17),
            moneda: r.get(18),
            cantidad: r.get(19),
            cantidad_fae: r.get(20),
            soles: r.get(21),
            dolares: r.get(22),
            precio_unitario: r.get(23),
            anho: r.get(24),
            mes: r.get(25),
            fecha_orig: r.get(26),
            fecha_ref: r.try_get(27).unwrap_or(None),
            fecha_venc: r.try_get(28).unwrap_or(None),
            cod_sucursal: r.get(29),
            nom_sucursal: r.get(30),
            departamento: r.get(31),
            provincia: r.get(32),
            distrito: r.get(33),
            id_vendedor: r.get(34),
            nom_vendedor: r.get(35),
            id_pedido: r.get(36),
            file_source: r.get(37),
            mes_ref: r.get(38),
            tipo_operacion: r.try_get(39).unwrap_or_default(),
            factura_ref_serie: r.try_get(40).unwrap_or_default(),
            factura_ref_nro: r.try_get(41).unwrap_or_default(),
            folio_unico: r.try_get(42).unwrap_or_default(),
        })
        .collect())
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
pub async fn refresh_stats_cache(pool: &SqlitePool) -> Result<()> {
    let (total_records,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas")
        .fetch_one(pool).await?;
    let (total_sales,): (f64,) = sqlx::query_as("SELECT COALESCE(SUM(soles), 0.0) FROM ventas")
        .fetch_one(pool).await?;
    let (total_clients,): (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT id_cliente) FROM ventas")
        .fetch_one(pool).await?;
    let (total_skus,): (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT original_sku) FROM ventas")
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

    // Vendedor: mismo numero con/sin prefijo "01"
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT CASE WHEN id_vendedor LIKE '01%' THEN SUBSTR(id_vendedor,3) ELSE id_vendedor END k,
                   COUNT(DISTINCT id_vendedor) n
            FROM ventas WHERE id_vendedor != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🔴 VENDEDORES PARTIDOS: {} grupos con forma corta y larga del mismo codigo", n));
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
