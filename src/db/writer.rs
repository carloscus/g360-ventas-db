// Comentarios en espanol - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use crate::config::db_path;
use crate::models::Venta;
use crate::db::schema::{CREATE_INDEXES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL, CREATE_STATS_CACHE_SQL};
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
    // IMPORTANTE: folio_unico = tpo_doc/serie_doc/nro_doc (POR DOCUMENTO).
    // Un documento puede tener varias lineas (distintos SKU). NO agrupar por
    // folio_unico solo: eso colapsaria facturas multi-linea a 1 fila (perdida real).
    // Dedup por linea completa: (doc, sku, cliente, fecha, cantidad, monto).
    let r = sqlx::query(
        "DELETE FROM ventas WHERE id NOT IN (
            SELECT MAX(id) FROM ventas
            GROUP BY folio_unico, id_articulo, id_cliente, tpo_doc, serie_doc, nro_doc, fecha_orig, cantidad, soles
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
