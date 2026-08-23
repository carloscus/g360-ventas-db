// Comentarios en espanol - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use super::schema::*;
use crate::config::db_path;
use crate::models::Venta;
use anyhow::{Context, Result};
use sqlx::{query, SqlitePool};
use tracing::info;

pub async fn init_pool() -> Result<SqlitePool> {
    let db = db_path();
    if let Some(p) = db.parent() {
        std::fs::create_dir_all(p)?;
    }
    let db_str = db.to_string_lossy().replace("\\", "/");
    let url = format!("sqlite://{}?mode=rwc", db_str);
    tracing::info!("Connecting to DB: {}", url);
    let pool = SqlitePool::connect(&url)
        .await
        .context("DB connect failed")?;
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await?;
    for col in [
        "doc_cliente TEXT",
        "precio_unitario REAL",
        "original_sku TEXT",
        "doc_std TEXT",
        "referencia_std TEXT",
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
    let _ = sqlx::query("UPDATE ventas SET precio_unitario = CASE WHEN cantidad != 0 THEN soles / cantidad ELSE 0 END WHERE precio_unitario IS NULL").execute(&pool).await;
    info!("DB ready: {}", db.display());
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
            "INSERT INTO ventas (id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo, id_tipo,nom_tipo,id_familia,nom_familia,estado_linea, id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia, moneda,cantidad,soles,dolares,precio_unitario,anho,mes,fecha_orig, fecha_ref,fecha_venc,fec_cargo,cod_sucursal,nom_sucursal, departamento,provincia,distrito,canal_dist,id_vendedor, nom_vendedor,id_pedido,file_source,mes_ref) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        )
        .bind(&v.id_articulo).bind(&v.original_sku).bind(&v.nom_articulo)
        .bind(&v.id_linea).bind(&v.nom_linea).bind(&v.id_grupo).bind(&v.nom_grupo)
        .bind(&v.id_tipo).bind(&v.nom_tipo).bind(&v.id_familia).bind(&v.nom_familia)
        .bind(&v.estado_linea).bind(&v.id_cliente).bind(&v.doc_cliente).bind(&v.nom_cliente)
        .bind(&v.tpo_doc).bind(&v.serie_doc).bind(&v.nro_doc).bind(&v.referencia)
        .bind(&v.moneda).bind(v.cantidad).bind(v.soles).bind(v.dolares).bind(v.precio_unitario)
        .bind(v.anho).bind(v.mes).bind(v.fecha_orig.to_string())
        .bind(&v.fecha_ref).bind(&v.fecha_venc).bind(&v.fec_cargo)
        .bind(&v.cod_sucursal).bind(&v.nom_sucursal).bind(&v.departamento).bind(&v.provincia).bind(&v.distrito)
        .bind(&v.canal_dist).bind(&v.id_vendedor).bind(&v.nom_vendedor).bind(&v.id_pedido)
        .bind(&v.file_source).bind(&v.mes_ref)
        .execute(&mut *tx).await?;
        n += 1;
    }
    tx.commit().await?;
    info!("Inserted {} rows", n);
    Ok(n)
}

pub async fn dedup_ventas(pool: &SqlitePool) -> Result<usize> {
    let r = sqlx::query("DELETE FROM ventas WHERE id NOT IN (SELECT MAX(id) FROM ventas GROUP BY id_articulo, id_cliente, tpo_doc, serie_doc, nro_doc, fecha_orig, soles)")
        .execute(pool).await?;
    Ok(r.rows_affected() as usize)
}

pub async fn count_ventas(pool: &SqlitePool) -> Result<i64> {
    let r: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas")
        .fetch_one(pool)
        .await?;
    Ok(r.0)
}

pub async fn fetch_all_ventas(pool: &SqlitePool) -> Result<Vec<Venta>, sqlx::Error> {
    use sqlx::Row;
    let rows = query("SELECT id_articulo, original_sku, nom_articulo, id_linea, nom_linea, id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia, estado_linea, id_cliente, doc_cliente, nom_cliente, tpo_doc, serie_doc, nro_doc, referencia, moneda, cantidad, soles, dolares, precio_unitario, anho, mes, fecha_orig, fecha_ref, fecha_venc, fec_cargo, cod_sucursal, nom_sucursal, departamento, provincia, distrito, canal_dist, id_vendedor, nom_vendedor, id_pedido, file_source, mes_ref FROM ventas")
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
            estado_linea: r.get(11),
            id_cliente: r.get(12),
            doc_cliente: r.get(13),
            nom_cliente: r.get(14),
            tpo_doc: r.get(15),
            serie_doc: r.get(16),
            nro_doc: r.get(17),
            referencia: r.get(18),
            moneda: r.get(19),
            cantidad: r.get(20),
            soles: r.get(21),
            dolares: r.get(22),
            precio_unitario: r.get(23),
            anho: r.get(24),
            mes: r.get(25),
            fecha_orig: r.get(26),
            fecha_ref: r.try_get(27).unwrap_or(None),
            fecha_venc: r.try_get(28).unwrap_or(None),
            fec_cargo: r.try_get(29).unwrap_or(None),
            cod_sucursal: r.get(30),
            nom_sucursal: r.get(31),
            departamento: r.get(32),
            provincia: r.get(33),
            distrito: r.get(34),
            canal_dist: r.get(35),
            id_vendedor: r.get(36),
            nom_vendedor: r.get(37),
            id_pedido: r.get(38),
            file_source: r.get(39),
            mes_ref: r.get(40),
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
