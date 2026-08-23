use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();
    info!("g360-db-ventas - Upload -> Supabase");
    let pool = g360_db_ventas::db::writer::init_pool().await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await?;
    if total == 0 {
        error!("No records");
        return Ok(());
    }
    info!("{} records", total);
    let bs = 500;
    let mut up = 0usize;
    for off in (0..total as usize).step_by(bs) {
        let rows: Vec<g360_db_ventas::models::Venta> = sqlx::query(
            "SELECT id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,\
             id_tipo,nom_tipo,id_familia,nom_familia,estado_linea,\
             id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,\
             moneda,cantidad,soles,dolares,precio_unitario,anho,mes,fecha_orig,\
             fecha_ref,fecha_venc,fec_cargo,cod_sucursal,nom_sucursal,\
             departamento,provincia,distrito,canal_dist,id_vendedor,\
             nom_vendedor,id_pedido,file_source,mes_ref \
             FROM ventas LIMIT ? OFFSET ?",
        )
        .bind(bs as i64)
        .bind(off as i64)
        .map(|row: sqlx::sqlite::SqliteRow| {
            use sqlx::Row;
            g360_db_ventas::models::Venta {
                id_articulo: row.get(0),
                original_sku: row.get(1),
                nom_articulo: row.get(2),
                id_linea: row.get(3),
                nom_linea: row.get(4),
                id_grupo: row.get(5),
                nom_grupo: row.get(6),
                id_tipo: row.get(7),
                nom_tipo: row.get(8),
                id_familia: row.get(9),
                nom_familia: row.get(10),
                estado_linea: row.get(11),
                id_cliente: row.get(12),
                doc_cliente: row.get(13),
                nom_cliente: row.get(14),
                tpo_doc: row.get(15),
                serie_doc: row.get(16),
                nro_doc: row.get(17),
                referencia: row.get(18),
                moneda: row.get(19),
                cantidad: row.get(20),
                soles: row.get(21),
                dolares: row.get(22),
                precio_unitario: row.get(23),
                anho: row.get(24),
                mes: row.get(25),
                fecha_orig: row.get(26),
                fecha_ref: row.get(27),
                fecha_venc: row.get(28),
                fec_cargo: row.get(29),
                cod_sucursal: row.get(30),
                nom_sucursal: row.get(31),
                departamento: row.get(32),
                provincia: row.get(33),
                distrito: row.get(34),
                canal_dist: row.get(35),
                id_vendedor: row.get(36),
                nom_vendedor: row.get(37),
                id_pedido: row.get(38),
                file_source: row.get(39),
                mes_ref: row.get(40),
            }
        })
        .fetch_all(&pool)
        .await?;
        match g360_db_ventas::processor::uploader::upload_to_supabase(&rows).await {
            Ok(n) => {
                up += n;
                info!("  {}/{}", up, total);
            }
            Err(e) => error!("  batch fail: {}", e),
        }
    }
    info!("Done: {}/{}", up, total);
    Ok(())
}
