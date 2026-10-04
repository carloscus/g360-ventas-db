// Comentarios en espanol - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use crate::config::db_path;
use crate::models::Venta;
use crate::db::schema::{CREATE_INDEXES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL, CREATE_STATS_CACHE_SQL, CREATE_AUDIT_TABLES_SQL, CREATE_DIM_TABLES_SQL};
use anyhow::{Context, Result};
use sqlx::{query, SqlitePool};
use std::sync::Mutex;
use tracing::info;

/// Columnas del INSERT de `ventas` (mismo orden que los binds de `insert_ventas`).
const INSERT_COLS: &str = "id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,id_tipo,nom_tipo,id_familia,nom_familia,id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,departamento,provincia,distrito,id_vendedor,nom_vendedor,id_pedido,ord_compra,file_source,mes_ref,tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico,id_ubigeo,canal_distribucion,nom_condicion_pago,estado_linea,division,fec_cargo,id_guia";

/// Pool singleton: evita recrear la conexión en cada llamada a init_pool().
/// sqlx::SqlitePool ya maneja internamente un pool de conexiones con WAL.
static POOL: Mutex<Option<SqlitePool>> = Mutex::new(None);

/// Cierra el pool singleton y lo deja para recrear. CRITICO antes de borrar
/// historial.db: en Windows un archivo abierto impide remove_file, y si el pool
/// sigue vivo init_pool() devuelve el mismo (con los datos). Deja el archivo libre.
pub async fn reset_pool() {
    let taken = { POOL.lock().unwrap().take() };
    if let Some(pool) = taken {
        pool.close().await;
    }
}

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
    // Aumentado 2026-09: con 2.8M+ filas las queries de integridad tardan >30s
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(120))
        // Tuning de rendimiento local (BD read-mostly de ~2.8M filas).
        // synchronous=NORMAL es seguro con WAL; temp_store/cache/mmap aceleran
        // agregaciones y lecturas del dashboard.
        .pragma("synchronous", "NORMAL")
        .pragma("temp_store", "MEMORY")
        .pragma("cache_size", "-64000")
        .pragma("mmap_size", "268435456")
        .pragma("wal_autocheckpoint", "2000")
        .pragma("journal_size_limit", "67108864");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(32)
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
    // PRAGMA optimize: aplica recomendaciones del planner (barato). Se complementa
    // con ANALYZE tras cargas masivas (ver analyze_db).
    sqlx::query("PRAGMA optimize").execute(&pool).await.ok();
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
        "id_ubigeo TEXT",
        "canal_distribucion TEXT",
        "nom_condicion_pago TEXT",
        "estado_linea TEXT",
        "division TEXT",
        "fec_cargo TEXT",
        "id_guia TEXT",
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
    // Migracion: stats_por_mes.cantidad (Bases existentes sin la columna)
    {
        let has: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('stats_por_mes') WHERE name='cantidad'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        if has == 0 {
            let _ = sqlx::query("ALTER TABLE stats_por_mes ADD COLUMN cantidad REAL NOT NULL DEFAULT 0")
                .execute(&pool)
                .await;
        }
    }
    // Auditoria/integridad + sync_months (reconciliacion resumible)
    let _ = sqlx::query(CREATE_AUDIT_TABLES_SQL).execute(&pool).await;
    // Dimension tables — derivadas de ventas, se refrescan tras cada cambio
    let _ = sqlx::query(CREATE_DIM_TABLES_SQL).execute(&pool).await;
    // Refresh stats cache on init
    let _ = refresh_stats_cache(&pool).await;
    // Dims: reconstruye solo si estan vacias o si cambio DIM_SCHEMA_VERSION
    let _ = ensure_dims_fresh(&pool).await;
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

pub async fn insert_ventas(pool: &SqlitePool, ventas: &[Venta], refresh_stats: bool) -> Result<usize> {
    // Sin filas: no borrar nada (una descarga/parseo vacio no debe vaciar el mes/dia).
    if ventas.is_empty() {
        return Ok(0);
    }
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
            // Guard de integridad: no reemplazar un mes si las filas nuevas son una
            // fraccion sospechosa de las existentes (descarga truncada / header cambiado).
            let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref)
                .fetch_one(&mut *tx)
                .await?;
            if existing > 1000 && (ventas.len() as f64) < (existing as f64) * 0.5 {
                return Err(anyhow::anyhow!(
                    "Guard de integridad: {} filas nuevas < 50% de {} existentes en '{}'. No se reemplaza el mes (posible descarga/parseo corrupto).",
                    ventas.len(), existing, first.mes_ref
                ));
            }
            sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref)
                .execute(&mut *tx)
                .await?;
        }
    }
    // Insercion en bloque (multi-row): 1 statement por chunk en vez de 1 por fila.
    // SQLite limita variables por statement (~32766); 51 cols x 400 = 20,400 < limite.
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

/// ANALYZE + optimize del planner tras cargas/borrados masivos (local, barato).
/// Mantiene las estadísticas al día para que SQLite elija buenos planes.
pub async fn analyze_db(pool: &SqlitePool) -> Result<()> {
    sqlx::query("ANALYZE").execute(pool).await.ok();
    sqlx::query("PRAGMA optimize").execute(pool).await.ok();
    Ok(())
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
    let (total_documents,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(DISTINCT folio_unico) FROM ventas {}", lf))
        .fetch_one(pool).await?;

    let now = chrono::Utc::now().to_rfc3339();
    let pairs = [
        ("total_records", total_records as f64),
        ("total_sales", total_sales),
        ("total_clients", total_clients as f64),
        ("total_skus", total_skus as f64),
        ("total_documents", total_documents as f64),
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
    // Agregado por mes precomputado (dashboard instantaneo, sin GROUP BY en vivo).
    if let Err(e) = refresh_stats_por_mes(pool).await {
        tracing::warn!("refresh_stats_por_mes fallo: {}", e);
    }
    Ok(())
}

/// Recomputa `stats_por_mes` (mismo filtro de lineas que el uploader). Se llama
/// tras cada carga/borrado masivo. El dashboard lee esta tabla en vez de agregar
/// 2.8M filas en cada poll.
pub async fn refresh_stats_por_mes(pool: &SqlitePool) -> Result<()> {
    let lf = crate::config::with_line_filter("");
    let rows: Vec<(String, i64, f64, i64, f64)> = sqlx::query_as(&format!(
        "SELECT mes_ref, COUNT(*) AS filas, COALESCE(SUM(soles), 0.0) AS soles, \
                COUNT(DISTINCT id_cliente) AS clientes, COALESCE(SUM(cantidad), 0.0) AS cantidad \
         FROM ventas {} GROUP BY mes_ref ORDER BY mes_ref",
        lf
    ))
    .fetch_all(pool)
    .await?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM stats_por_mes").execute(&mut *tx).await?;
    for (mes, filas, soles, clientes, cantidad) in &rows {
        sqlx::query(
            "INSERT INTO stats_por_mes (mes_ref, filas, ventas_soles, clientes, cantidad, actualizado_en) \
             VALUES (?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind(mes)
        .bind(filas)
        .bind(soles)
        .bind(clientes)
        .bind(cantidad)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Reconstruye todas las tablas de dimensiones desde la tabla ventas (fact table).
/// Cada dimensión se procesa en una funcion separada para trazabilidad de errores.
/// Se llama tras cada dedup/insert/migracion para mantener consistencia (SIEMPRE
/// reconstruye). Para el arranque caliente usar `ensure_dims_fresh`.
/// Costo estimado: ~2-5s en BD de 1.4M filas (full scan de ventas, pero con indices).
pub async fn refresh_dim_tables(pool: &SqlitePool) -> Result<usize> {
    let n_cli = refresh_dim_cliente(pool).await?;
    let n_vend = refresh_dim_vendedor(pool).await?;
    let n_art = refresh_dim_articulo(pool).await?;
    let n_doc = refresh_dim_documento(pool).await?;
    let n_line = refresh_dim_linea(pool).await?;
    let n_fact = refresh_fact_venta_mes(pool).await?;
    info!(
        "dim_tables rebuild: cliente={}, vendedor={}, articulo={}, documento={}, linea={}, fact_venta_mes={}",
        n_cli, n_vend, n_art, n_doc, n_line, n_fact
    );
    Ok(n_cli + n_vend + n_art + n_doc + n_line + n_fact)
}

/// Reconstruye `fact_venta_mes`: agregado mensual por linea+SKU que alimenta los
/// comparativos (vw_comparativo_ventas / vw_comparativo_linea). Precomputarlo
/// evita reagregar 2.8M de filas en cada consulta (~60s -> ms).
/// Convencion: `devuelto` es magnitud positiva; `soles_netos` = ventas - NC/ND.
async fn refresh_fact_venta_mes(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM fact_venta_mes")
        .execute(pool)
        .await
        .context("refresh_fact_venta_mes: DELETE falló")?;
    let n = sqlx::query(r#"
        INSERT INTO fact_venta_mes
            (id_linea, id_articulo, mes_ref, cantidad, soles, devuelto, soles_netos)
        SELECT
            id_linea, id_articulo, mes_ref,
            SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END) AS cantidad,
            ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS soles,
            SUM(CASE WHEN tipo_operacion = 'devolucion' THEN ABS(cantidad) ELSE 0 END) AS devuelto,
            ROUND(SUM(soles), 2) AS soles_netos
        FROM ventas
        WHERE id_articulo != '' AND id_linea != ''
        GROUP BY id_linea, id_articulo, mes_ref
    "#)
    .execute(pool)
    .await
    .context("refresh_fact_venta_mes: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Reconstruye las dims SOLO si (a) estan vacias (BD nueva/heredada) o
/// (b) `PRAGMA user_version` < `DIM_SCHEMA_VERSION` (cambio de semantica).
/// Al reconstruir actualiza el marcador. Barato cuando ya esta al dia: evita
/// reconstruir en cada poll de la UI (init_pool se invoca cada ~2s).
pub async fn ensure_dims_fresh(pool: &SqlitePool) -> Result<usize> {
    let user_version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let dims_vacias: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dim_cliente")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let necesita = dims_vacias == 0 || user_version < crate::db::schema::DIM_SCHEMA_VERSION;
    if !necesita {
        return Ok(0);
    }

    let n = refresh_dim_tables(pool).await?;
    // PRAGMA no admite bind de parametros; el valor es una constante controlada.
    let _ = sqlx::query(&format!(
        "PRAGMA user_version = {}",
        crate::db::schema::DIM_SCHEMA_VERSION
    ))
    .execute(pool)
    .await;
    if user_version < crate::db::schema::DIM_SCHEMA_VERSION && dims_vacias > 0 {
        info!(
            "dim_tables migradas a version {} (antes {}): {} filas",
            crate::db::schema::DIM_SCHEMA_VERSION, user_version, n
        );
    }
    Ok(n)
}

/// Refresca dim_cliente: ultima info conocida por cliente + KPIs agregados.
/// Fuente de verdad para navegacion por cliente en el dashboard local.
///
/// CLAVE ESTABLE = id_cliente. Los atributos (nom_cliente, doc_cliente=RUC,
/// departamento, ...) se toman de la fila MAS RECIENTE, no del MAX() alfabetico:
/// si el ERP renombra a un cliente, la dimension muestra el nombre vigente.
/// Mismo criterio que `vw_dim_cliente`.
async fn refresh_dim_cliente(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_cliente")
        .execute(pool)
        .await
        .context("refresh_dim_cliente: DELETE falló")?;
    let n = sqlx::query(r#"
        WITH reciente AS (
            SELECT id_cliente, nom_cliente, doc_cliente, departamento, provincia, distrito,
                   ROW_NUMBER() OVER (
                       PARTITION BY id_cliente
                       ORDER BY CASE WHEN nom_cliente != '' THEN 0 ELSE 1 END,
                                fecha_orig DESC, id DESC
                   ) AS rn
            FROM ventas WHERE id_cliente != ''
        ),
        agg AS (
            SELECT id_cliente,
                   COUNT(*)           AS n_ventas,
                   ROUND(SUM(soles), 2)         AS total_soles,
                   ROUND(SUM(COALESCE(dolares, 0.0)), 2) AS total_dolares,
                   MAX(fecha_orig)    AS ultima_compra
            FROM ventas WHERE id_cliente != ''
            GROUP BY id_cliente
        )
        INSERT INTO dim_cliente
            (id_cliente, nom_cliente, doc_cliente, departamento, provincia, distrito,
             n_ventas, total_soles, total_dolares, ultima_compra, actualizado_en)
        SELECT r.id_cliente, r.nom_cliente, r.doc_cliente, r.departamento, r.provincia, r.distrito,
               a.n_ventas, a.total_soles, a.total_dolares, a.ultima_compra, datetime('now')
        FROM reciente r
        JOIN agg a ON a.id_cliente = r.id_cliente
        WHERE r.rn = 1
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_cliente: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Refresca dim_vendedor: ultima info por vendedor + KPIs agregados.
/// Permite navegar por vendedores y ver su cartera de clientes rapidamente.
/// Atributos por fila MAS RECIENTE (igual que dim_cliente).
async fn refresh_dim_vendedor(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_vendedor")
        .execute(pool)
        .await
        .context("refresh_dim_vendedor: DELETE falló")?;
    let n = sqlx::query(r#"
        WITH reciente AS (
            SELECT id_vendedor, nom_vendedor,
                   ROW_NUMBER() OVER (
                       PARTITION BY id_vendedor
                       ORDER BY CASE WHEN nom_vendedor != '' THEN 0 ELSE 1 END,
                                fecha_orig DESC, id DESC
                   ) AS rn
            FROM ventas WHERE id_vendedor != ''
        ),
        agg AS (
            SELECT id_vendedor,
                   COUNT(*)              AS n_ventas,
                   ROUND(SUM(soles), 2)  AS total_soles,
                   COUNT(DISTINCT id_cliente) AS n_clientes,
                   MAX(fecha_orig)       AS ultima_venta
            FROM ventas WHERE id_vendedor != ''
            GROUP BY id_vendedor
        )
        INSERT INTO dim_vendedor
            (id_vendedor, nom_vendedor, n_ventas, total_soles, n_clientes, ultima_venta, actualizado_en)
        SELECT r.id_vendedor, r.nom_vendedor,
               a.n_ventas, a.total_soles, a.n_clientes, a.ultima_venta, datetime('now')
        FROM reciente r
        JOIN agg a ON a.id_vendedor = r.id_vendedor
        WHERE r.rn = 1
    "#)
    .execute(pool)
    .await
    .context("refresh_dim_vendedor: INSERT-SELECT falló")?;
    Ok(n.rows_affected() as usize)
}

/// Refresca dim_articulo: ultima info de catalogo por SKU + KPIs agregados.
/// Permite navegar por articulos, ver precio promedio y volumen de ventas.
///
/// CLAVE ESTABLE = id_articulo (SKU). Los atributos de catalogo (nom_articulo,
/// id_linea/nom_linea, grupo, tipo, familia) se toman de la fila MAS RECIENTE:
/// si el ERP renombra un producto SIN cambiar el SKU, la dimension muestra el
/// nombre vigente. Mismo criterio que `vw_dim_articulo`.
async fn refresh_dim_articulo(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_articulo")
        .execute(pool)
        .await
        .context("refresh_dim_articulo: DELETE falló")?;
    let n = sqlx::query(r#"
        WITH reciente AS (
            SELECT id_articulo, original_sku, nom_articulo,
                   id_linea, nom_linea, id_grupo, nom_grupo,
                   id_tipo, nom_tipo, id_familia, nom_familia,
                   ROW_NUMBER() OVER (
                       PARTITION BY id_articulo
                       ORDER BY CASE WHEN nom_articulo != '' THEN 0 ELSE 1 END,
                                fecha_orig DESC, id DESC
                   ) AS rn
            FROM ventas WHERE id_articulo != ''
        ),
        agg AS (
            SELECT id_articulo,
                   COUNT(*)            AS n_ventas,
                   ROUND(SUM(cantidad), 2) AS total_cantidad,
                   ROUND(SUM(soles), 2)      AS total_soles,
                   ROUND(SUM(soles) / NULLIF(SUM(cantidad), 0), 4) AS precio_promedio,
                   MAX(fecha_orig)     AS ultima_venta
            FROM ventas WHERE id_articulo != ''
            GROUP BY id_articulo
        )
        INSERT INTO dim_articulo
            (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
             id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia,
             n_ventas, total_cantidad, total_soles, precio_promedio, ultima_venta, actualizado_en)
        SELECT r.id_articulo, r.original_sku, r.nom_articulo,
               r.id_linea, r.nom_linea, r.id_grupo, r.nom_grupo,
               r.id_tipo, r.nom_tipo, r.id_familia, r.nom_familia,
               a.n_ventas, a.total_cantidad, a.total_soles, a.precio_promedio,
               a.ultima_venta, datetime('now')
        FROM reciente r
        JOIN agg a ON a.id_articulo = r.id_articulo
        WHERE r.rn = 1
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

/// Refresca dim_linea: ultima info por linea/negocio + KPIs agregados.
/// Permite navegar por lineas de producto y ver volumen por grupo/familia.
///
/// CLAVE ESTABLE = id_linea. nom_linea/nom_grupo/nom_familia se toman de la
/// fila MAS RECIENTE: si el ERP renombra una linea SIN cambiar el codigo, la
/// dimension muestra el nombre vigente (mismo criterio que dim_articulo).
async fn refresh_dim_linea(pool: &SqlitePool) -> Result<usize> {
    sqlx::query("DELETE FROM dim_linea")
        .execute(pool)
        .await
        .context("refresh_dim_linea: DELETE falló")?;
    let n = sqlx::query(r#"
        WITH reciente AS (
            SELECT id_linea, nom_linea, id_grupo, nom_grupo, id_familia, nom_familia,
                   ROW_NUMBER() OVER (
                       PARTITION BY id_linea
                       ORDER BY CASE WHEN nom_linea != '' THEN 0 ELSE 1 END,
                                fecha_orig DESC, id DESC
                   ) AS rn
            FROM ventas WHERE id_linea != ''
        ),
        agg AS (
            SELECT id_linea,
                   COUNT(*)              AS n_ventas,
                   ROUND(SUM(soles), 2)  AS total_soles,
                   COUNT(DISTINCT id_cliente) AS n_clientes
            FROM ventas WHERE id_linea != ''
            GROUP BY id_linea
        )
        INSERT INTO dim_linea
            (id_linea, nom_linea, id_grupo, nom_grupo, id_familia, nom_familia,
             n_ventas, total_soles, n_clientes, actualizado_en)
        SELECT r.id_linea, r.nom_linea, r.id_grupo, r.nom_grupo, r.id_familia, r.nom_familia,
               a.n_ventas, a.total_soles, a.n_clientes, datetime('now')
        FROM reciente r
        JOIN agg a ON a.id_linea = r.id_linea
        WHERE r.rn = 1
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

    // SKU: colisiones al quitar ceros — INFORMATIVO / FALSO POSITIVO conocido.
    // Verificado 2026-09-15: los 82 grupos son productos DISTINTOS en lineas
    // distintas (ej: 011019=PELOTAS L01 vs 11019=METALICA L11). El cero inicial
    // es significativo. Solo alertar si el conteo crece de forma inesperada.
    // El parser preserva el SKU tal cual (clean_id no hace zero-fill/strip); el
    // unico riesgo real (import manual de XLS con columna numerica) esta cubierto
    // por `xls::inspect_sku_column` + gate en import_manual_*.
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
            "ℹ️  SKUs con colision de ceros: {} grupos (falso positivo esperado: 011019≠11019, lineas distintas; revisar si crece)", n));
    }

    // Mismo RUC (doc_cliente) apuntando a 2+ id_cliente — excluye anomalias ERP conocidas.
    // PROPIO DE LA FUENTE (renombres/fusiones del ERP): la clave id_cliente no se
    // parte; solo se informa para auditoria. El pipeline no lo corrige.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT doc_cliente) FROM (
            SELECT doc_cliente, COUNT(DISTINCT id_cliente) as n
            FROM ventas
            WHERE doc_cliente NOT IN ('', '00') AND LENGTH(doc_cliente) >= 10
            GROUP BY doc_cliente HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🟡 RUC duplicado: {} RUCs apuntan a 2+ id_cliente (fuente ERP: renombres/fusiones; no accionable)", n));
    }

    // Mismo nombre de cliente con 2+ id_cliente (causa comun de splits).
    // PROPIO DE LA FUENTE (split por renombre ERP); vw_dim_cliente muestra el
    // nombre mas reciente, las agregaciones por id_cliente no se parten.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT UPPER(TRIM(nom_cliente)) k, COUNT(DISTINCT id_cliente) n
            FROM ventas WHERE nom_cliente != '' GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;
    if n > 0 {
        issues.push(format!("🟡 Nombres repetidos: {} nombres de cliente comparten 2+ id_cliente (fuente ERP: split por renombre; no accionable)", n));
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

    // NOTA: NO se valida unicidad por (folio_unico, id_articulo). Esa clave NO es
    // unica en el ERP: una factura puede repetir el mismo SKU en varias lineas
    // (gratuita + pagada, precios distintos, repeticion por guia de remision).
    // Se conservan TODAS las filas; no es una inconsistencia.

    // 1. Verificar facturas sin líneas (folio vacío)
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
    // Forzar rebuild de dims/fact para que el export tenga dimensiones vigentes
    // (las dims son cache: los consumidores online usan las vistas vw_dim_*).
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

/// Exporta una INSTANTANEA canonica para que otra app trabaje sobre una copia
/// aislada: `VACUUM INTO` (archivo .db autocontenido, sin WAL) + un manifiesto JSON
/// con versiones, cobertura y checksums mensuales (baseline de validacion).
pub async fn export_canonical_snapshot(target: &std::path::Path) -> Result<serde_json::Value> {
    let pool = init_pool().await?;
    // Dims frescas pero SIN reconstruir si ya estan al dia (ensure_dims_fresh es
    // barato cuando no hace falta; refresh_dim_tables reconstruye ~2.8M filas y
    // hacia el export lentisimo). Los checksums faltantes se completan.
    let _ = ensure_dims_fresh(&pool).await;
    let _ = recalculate_missing_checksums(&pool).await;
    let _ = sqlx::query("VACUUM INTO ?")
        .bind(target.to_string_lossy().to_string())
        .execute(&pool)
        .await?;
    let size = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);

    let total_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let mm: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT MIN(mes_ref), MAX(mes_ref), COUNT(DISTINCT mes_ref) \
         FROM ventas WHERE length(mes_ref)=7",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or((None, None, 0));
    let checksums: Vec<(String, String, i64, f64)> = sqlx::query_as(
        "SELECT mes_ref, checksum, total_filas, total_soles \
         FROM mes_checksums ORDER BY mes_ref",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let manifest = serde_json::json!({
        "format": "g360-ventas-canonical",
        "app": "g360-db-ventas",
        "app_version": env!("CARGO_PKG_VERSION"),
        "schema_version": crate::db::schema::DIM_SCHEMA_VERSION,
        "exported_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "db_file": target.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        "db_size_bytes": size,
        "total_rows_raw": total_rows,
        "min_mes": mm.0,
        "max_mes": mm.1,
        "total_months": mm.2,
        "checksum_months": checksums.len(),
        "checksums": checksums.iter().map(|(m, c, f, s)| serde_json::json!({
            "mes_ref": m, "checksum": c, "filas": f, "soles": s
        })).collect::<Vec<_>>(),
        "filters": "raw-first: TODAS las lineas del ERP; sin filtro de lineas ni retencion",
        "consumer_hint": "Copia el .db a la app consumidora y valida conteos/checksums por mes contra este manifiesto antes de usarla."
    });
    let manifest_path = target.with_extension("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    info!(
        "Snapshot canonico: {} ({:.1} MB) + manifiesto {}",
        target.display(),
        size as f64 / 1024.0 / 1024.0,
        manifest_path.display()
    );

    Ok(serde_json::json!({
        "db_path": target.to_string_lossy().to_string(),
        "manifest_path": manifest_path.to_string_lossy().to_string(),
        "db_size_mb": (size as f64) / 1024.0 / 1024.0,
        "total_rows_raw": total_rows,
        "min_mes": mm.0,
        "max_mes": mm.1,
        "total_months": mm.2,
        "checksum_months": checksums.len(),
    }))
}

/// Métricas de la BD local para la sección "DB Local" del dashboard.
pub async fn local_metrics(pool: &SqlitePool) -> Result<serde_json::Value> {
    let stat = |key: &'static str| async move {
        sqlx::query_scalar::<_, f64>("SELECT value FROM stats_cache WHERE key = ?")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap_or(0.0)
    };
    let min_date: Option<String> = sqlx::query_scalar("SELECT MIN(fecha_orig) FROM ventas")
        .fetch_one(pool).await.unwrap_or(None);
    let max_date: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
        .fetch_one(pool).await.unwrap_or(None);
    let total_months: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM ventas")
        .fetch_one(pool).await.unwrap_or(0);
    let last_checksum: Option<String> = sqlx::query_scalar("SELECT MAX(calculado_en) FROM mes_checksums")
        .fetch_one(pool).await.unwrap_or(None);
    // Relación de folios NC/NDB: total, asociadas a su factura y huérfanas.
    let ncndb_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE tpo_doc IN ('NCR','NDB')")
        .fetch_one(pool).await.unwrap_or(0);
    let ncndb_orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ventas v WHERE v.tpo_doc IN ('NCR','NDB') \
         AND NOT EXISTS (SELECT 1 FROM ventas f WHERE f.serie_doc = v.factura_ref_serie AND f.nro_doc = v.factura_ref_nro)",
    )
    .fetch_one(pool).await.unwrap_or(0);
    Ok(serde_json::json!({
        "min_date": min_date.unwrap_or_default(),
        "max_date": max_date.unwrap_or_default(),
        "total_records": stat("total_records").await as i64,
        "total_sales": stat("total_sales").await,
        "total_clients": stat("total_clients").await as i64,
        "total_skus": stat("total_skus").await as i64,
        "total_documents": stat("total_documents").await as i64,
        "total_months": total_months,
        "last_checksum": last_checksum.unwrap_or_default(),
        "ncndb_total": ncndb_total,
        "ncndb_linked": (ncndb_total - ncndb_orphans).max(0),
        "ncndb_orphans": ncndb_orphans,
    }))
}

/// Reporte de calidad de datos con severidad. Devuelve un JSON con listas
/// `critical` (rompe consistencia), `warn` (anomalías de la fuente) e `info`.
pub async fn data_quality_report(pool: &SqlitePool) -> Result<serde_json::Value> {
    use chrono::Datelike as _;
    let mut critical: Vec<String> = Vec::new();
    let mut warn: Vec<String> = Vec::new();
    let mut info: Vec<String> = Vec::new();

    let count = |sql: String| async move {
        sqlx::query_scalar::<_, i64>(&sql)
            .fetch_one(pool)
            .await
            .unwrap_or(0)
    };

    // ── Crítico: campos requeridos vacíos ──
    for (c, lbl) in [
        ("id_articulo", "SKU"),
        ("id_cliente", "cliente"),
        ("tpo_doc", "tipo de documento"),
        ("serie_doc", "serie"),
        ("nro_doc", "número"),
        ("fecha_orig", "fecha"),
        ("folio_unico", "folio"),
        ("mes_ref", "mes"),
    ] {
        let n = count(format!(
            "SELECT COUNT(*) FROM ventas WHERE {} IS NULL OR TRIM({})=''",
            c, c
        ))
        .await;
        if n > 0 {
            critical.push(format!("{} filas sin {} ({})", n, lbl, c));
        }
    }
    // ── Crítico: fechas inválidas / mes_ref inconsistente ──
    let n = count("SELECT COUNT(*) FROM ventas WHERE fecha_orig NOT GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'".into()).await;
    if n > 0 {
        critical.push(format!("{} filas con fecha_orig inválida", n));
    }
    let n = count("SELECT COUNT(*) FROM ventas WHERE length(mes_ref)=7 AND mes_ref != substr(fecha_orig,1,7)".into()).await;
    if n > 0 {
        critical.push(format!("{} filas con mes_ref distinto de fecha_orig", n));
    }

    // ── Warn: anomalías de la fuente (ERP) ──
    let n = count("SELECT COUNT(*) FROM ventas WHERE tpo_doc IN ('NCR','NDB') AND (factura_ref_serie='' OR factura_ref_nro='')".into()).await;
    if n > 0 {
        warn.push(format!("{} NC/NDB sin factura de referencia", n));
    }
    let n = count("SELECT COUNT(*) FROM ventas WHERE precio_unitario=0 AND soles!=0".into()).await;
    if n > 0 {
        warn.push(format!("{} filas con precio_unitario=0 pero monto distinto de 0", n));
    }
    let n = count("SELECT COUNT(*) FROM ventas WHERE soles<0 AND tipo_operacion='venta'".into()).await;
    if n > 0 {
        warn.push(format!("{} ventas con monto negativo", n));
    }
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let n = count(format!("SELECT COUNT(*) FROM ventas WHERE fecha_orig > '{}'", today)).await;
    if n > 0 {
        warn.push(format!("{} filas con fecha futura (>{})", n, today));
    }
    // Huecos de meses
    let minmax: (Option<String>, Option<String>, i64) =
        sqlx::query_as("SELECT MIN(mes_ref), MAX(mes_ref), COUNT(DISTINCT mes_ref) FROM ventas WHERE length(mes_ref)=7")
            .fetch_one(pool)
            .await
            .unwrap_or((None, None, 0));
    if let (Some(mn), Some(mx)) = (&minmax.0, &minmax.1) {
        if let (Ok(a), Ok(b)) = (
            chrono::NaiveDate::parse_from_str(&format!("{}-01", mn), "%Y-%m-%d"),
            chrono::NaiveDate::parse_from_str(&format!("{}-01", mx), "%Y-%m-%d"),
        ) {
            let months = (b.year() - a.year()) * 12 + (b.month() as i32 - a.month() as i32) + 1;
            if (months as i64) > minmax.2 {
                warn.push(format!(
                    "Huecos temporales: {} de {} meses presentes entre {} y {}",
                    months as i64 - minmax.2,
                    months,
                    mn,
                    mx
                ));
            }
        }
    }

    // ── Info: notas no bloqueantes ──
    let n = count("SELECT COUNT(*) FROM ventas v WHERE v.tpo_doc IN ('NCR','NDB') AND NOT EXISTS (SELECT 1 FROM ventas f WHERE f.serie_doc=v.factura_ref_serie AND f.nro_doc=v.factura_ref_nro)".into()).await;
    if n > 0 {
        info.push(format!("{} NC/NDB que referencian documentos fuera de la BD (pre-2018/excluidos)", n));
    }
    let n = count("SELECT COUNT(*) FROM (SELECT folio_unico, id_articulo FROM ventas GROUP BY folio_unico, id_articulo HAVING COUNT(*)>1)".into()).await;
    if n > 0 {
        info.push(format!("{} documentos con el mismo SKU en varias líneas (legítimo)", n));
    }
    let n = count("SELECT COUNT(*) FROM ventas WHERE cantidad=0".into()).await;
    if n > 0 {
        info.push(format!("{} filas con cantidad=0 (ajustes de valor)", n));
    }
    let checksum_count = count("SELECT COUNT(*) FROM mes_checksums".into()).await;
    info.push(format!(
        "Cobertura: {} a {} · {} meses con datos · {} meses con checksum",
        minmax.0.unwrap_or_default(),
        minmax.1.unwrap_or_default(),
        minmax.2,
        checksum_count
    ));

    Ok(serde_json::json!({
        "critical": critical,
        "warn": warn,
        "info": info,
        "generated_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }))
}

/// Exporta por separado las NCR y NDB a CSV, con columnas del documento y el
/// vínculo a la factura referenciada (LEFT JOIN por serie/nro).
/// Rango por `mes_ref` (YYYY-MM). Escribe en `<data>/export/`.
pub async fn export_nc_ndb(pool: &SqlitePool, desde: &str, hasta: &str) -> Result<Vec<String>> {
    let dir = crate::config::data_dir().join("export");
    std::fs::create_dir_all(&dir)?;
    let mut out: Vec<String> = Vec::new();

    let sql = r#"SELECT
            v.folio_unico, v.fecha_orig, v.mes_ref,
            v.id_cliente, v.nom_cliente, v.doc_cliente,
            v.id_articulo, v.nom_articulo, v.id_linea, v.nom_linea,
            v.cantidad, v.soles, v.moneda, v.tipo_operacion,
            v.factura_ref_serie, v.factura_ref_nro,
            f.ref_fecha, f.ref_tpo, f.ref_soles
        FROM ventas v
        LEFT JOIN (
            SELECT serie_doc, nro_doc,
                   MIN(fecha_orig) AS ref_fecha,
                   MIN(tpo_doc) AS ref_tpo,
                   ROUND(SUM(soles), 2) AS ref_soles
            FROM ventas
            GROUP BY serie_doc, nro_doc
        ) f ON f.serie_doc = v.factura_ref_serie AND f.nro_doc = v.factura_ref_nro
        WHERE v.tpo_doc = ? AND v.mes_ref >= ? AND v.mes_ref <= ?
        ORDER BY v.fecha_orig, v.serie_doc, v.nro_doc, v.id_articulo"#;

    for (tpo, name, label) in [("NCR", "nc", "Notas de Crédito"), ("NDB", "ndb", "Notas de Débito")] {
        let path = dir.join(format!("{}_{}_{}.csv", name, desde, hasta));
        let mut w = csv::Writer::from_path(&path)?;
        w.write_record([
            "folio_unico", "fecha_orig", "mes_ref",
            "id_cliente", "nom_cliente", "doc_cliente",
            "id_articulo", "nom_articulo", "id_linea", "nom_linea",
            "cantidad", "soles", "moneda", "tipo_operacion",
            "factura_ref_serie", "factura_ref_nro",
            "ref_fecha_orig", "ref_tpo_doc", "ref_soles",
        ])?;

        let rows = sqlx::query(sql)
            .bind(tpo)
            .bind(desde)
            .bind(hasta)
            .fetch_all(pool)
            .await?;
        let mut n = 0usize;
        for r in &rows {
            use sqlx::Row;
            let gs = |i: usize| -> String {
                r.try_get::<Option<String>, _>(i).ok().flatten().unwrap_or_default()
            };
            let gf = |i: usize| -> f64 { r.try_get::<Option<f64>, _>(i).ok().flatten().unwrap_or(0.0) };
            w.write_record([
                &gs(0), &gs(1), &gs(2), &gs(3), &gs(4), &gs(5), &gs(6), &gs(7), &gs(8), &gs(9),
                &gf(10).to_string(), &gf(11).to_string(), &gs(12), &gs(13), &gs(14), &gs(15),
                &gs(16), &gs(17), &gf(18).to_string(),
            ])?;
            n += 1;
        }
        w.flush()?;
        info!("Export {} ({}): {} filas -> {}", label, tpo, n, path.display());
        out.push(path.to_string_lossy().to_string());
    }
    Ok(out)
}

/// Monitor mensual para comparar contra la intranet: mes | filas | unidades | total soles.
/// Mismo filtro de líneas que el uploader (coherente con Supabase).
pub async fn monthly_monitor(pool: &SqlitePool) -> Result<Vec<(String, i64, f64, f64)>> {
    let rows = sqlx::query_as(
        "SELECT mes_ref, filas, cantidad, ventas_soles FROM stats_por_mes ORDER BY mes_ref DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn xlsx_col(i: usize) -> String {
    let mut n = i + 1;
    let mut s = String::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        s.insert(0, (b'A' + r) as char);
        n = (n - 1) / 26;
    }
    s
}

fn xlsx_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Escribe un XLSX mínimo (OOXML) con la tabla del monitor. Usa el crate `zip`
/// (ya presente por calamine) para no agregar dependencias pesadas.
fn write_monitor_xlsx(path: &std::path::Path, rows: &[(String, i64, f64, f64)]) -> Result<()> {
    use std::io::Write;
    let headers = ["mes", "filas", "unidades", "total_soles"];

    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    // Fila de encabezados
    sheet.push_str("<row r=\"1\">");
    for (ci, h) in headers.iter().enumerate() {
        sheet.push_str(&format!(
            "<c r=\"{}{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
            xlsx_col(ci), 1, h
        ));
    }
    sheet.push_str("</row>");
    // Filas de datos
    for (ri, (mes, filas, cantidad, soles)) in rows.iter().enumerate() {
        let r = ri + 2;
        sheet.push_str(&format!("<row r=\"{}\">", r));
        sheet.push_str(&format!(
            "<c r=\"A{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
            r,
            xlsx_escape(mes)
        ));
        sheet.push_str(&format!("<c r=\"B{}\"><v>{}</v></c>", r, filas));
        sheet.push_str(&format!("<c r=\"C{}\"><v>{:.0}</v></c>", r, cantidad));
        sheet.push_str(&format!("<c r=\"D{}\"><v>{:.2}</v></c>", r, soles));
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
</Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>";
    let workbook = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<sheets><sheet name=\"Monitor\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";
    let workbook_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
</Relationships>";

    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in [
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", workbook_rels),
        ("xl/worksheets/sheet1.xml", sheet.as_str()),
    ] {
        zip.start_file(name, opts)?;
        zip.write_all(content.as_bytes())?;
    }
    zip.finish()?;
    Ok(())
}

/// Exporta el monitor mensual a XLSX en `<data>/export/monitor_<ts>.xlsx`.
pub async fn export_monitor_xlsx(pool: &SqlitePool) -> Result<String> {
    let rows = monthly_monitor(pool).await?;
    let dir = crate::config::data_dir().join("export");
    std::fs::create_dir_all(&dir)?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let path = dir.join(format!("monitor_{}.xlsx", ts));
    write_monitor_xlsx(&path, &rows)?;
    info!("Monitor XLSX exportado: {} ({} meses)", path.display(), rows.len());
    Ok(path.to_string_lossy().to_string())
}
