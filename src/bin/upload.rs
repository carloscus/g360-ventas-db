use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();
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
             id_tipo,nom_tipo,id_familia,nom_familia,\
             id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,\
             moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,\
             fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,\
             departamento,provincia,distrito,id_vendedor,\
             nom_vendedor,id_pedido,file_source,mes_ref,\
             tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico \
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
                id_cliente: row.get(11),
                doc_cliente: row.get(12),
                nom_cliente: row.get(13),
                tpo_doc: row.get(14),
                serie_doc: row.get(15),
                nro_doc: row.get(16),
                referencia: row.get(17),
                moneda: row.get(18),
                cantidad: row.get(19),
                cantidad_fae: row.try_get(20).unwrap_or_default(),
                soles: row.get(21),
                dolares: row.get(22),
                precio_unitario: row.get(23),
                anho: row.get(24),
                mes: row.get(25),
                fecha_orig: row.get(26),
                fecha_ref: row.get(27),
                fecha_venc: row.get(28),
                cod_sucursal: row.get(29),
                nom_sucursal: row.get(30),
                departamento: row.get(31),
                provincia: row.get(32),
                distrito: row.get(33),
                id_vendedor: row.get(34),
                nom_vendedor: row.get(35),
                id_pedido: row.get(36),
                file_source: row.get(37),
                mes_ref: row.get(38),
                tipo_operacion: row.try_get(39).unwrap_or_default(),
                factura_ref_serie: row.try_get(40).unwrap_or_default(),
                factura_ref_nro: row.try_get(41).unwrap_or_default(),
                folio_unico: row.try_get(42).unwrap_or_default(),
            }
        })
        .fetch_all(&pool)
        .await?;
        match g360_db_ventas::processor::uploader::upload_to_supabase(&rows, &None).await {
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
