use crate::models::Venta;

mod info;
mod purge;
mod reconcile;
mod sync;

pub use info::*;
pub use purge::*;
pub use reconcile::*;
pub use sync::*;

/// Columnas de `ventas` en el orden EXACTO que espera `map_venta_row`.
const VENTA_SELECT_COLS: &str = "id, id_articulo,original_sku,nom_articulo,id_linea,nom_linea,id_grupo,nom_grupo,\
id_tipo,nom_tipo,id_familia,nom_familia,\
id_cliente,doc_cliente,nom_cliente,tpo_doc,serie_doc,nro_doc,referencia,\
moneda,cantidad,cantidad_fae,soles,dolares,precio_unitario,anho,mes,fecha_orig,\
fecha_ref,fecha_venc,cod_sucursal,nom_sucursal,\
departamento,provincia,distrito,id_vendedor,\
nom_vendedor,id_pedido,ord_compra,file_source,mes_ref,\
tipo_operacion,factura_ref_serie,factura_ref_nro,folio_unico";

/// Mapea una fila de `ventas` (SELECT con el orden de columnas de `VENTA_SELECT_COLS`)
/// a `(id, Venta)`. Reutilizable por upload_all y reconcile_supabase.
fn map_venta_row(row: sqlx::sqlite::SqliteRow) -> (i64, Venta) {
    use sqlx::Row;
    let v = Venta {
        id_articulo: row.get(1),
        original_sku: row.get(2),
        nom_articulo: row.get(3),
        id_linea: row.get(4),
        nom_linea: row.get(5),
        id_grupo: row.get(6),
        nom_grupo: row.get(7),
        id_tipo: row.get(8),
        nom_tipo: row.get(9),
        id_familia: row.get(10),
        nom_familia: row.get(11),
        id_cliente: row.get(12),
        doc_cliente: row.get(13),
        nom_cliente: row.get(14),
        tpo_doc: row.get(15),
        serie_doc: row.get(16),
        nro_doc: row.get(17),
        referencia: row.get(18),
        moneda: row.get(19),
        cantidad: row.get(20),
        cantidad_fae: row.try_get(21).unwrap_or_default(),
        soles: row.get(22),
        dolares: row.get(23),
        precio_unitario: row.get(24),
        anho: row.get(25),
        mes: row.get(26),
        fecha_orig: row.get(27),
        fecha_ref: row.try_get(28).unwrap_or(None),
        fecha_venc: row.try_get(29).unwrap_or(None),
        cod_sucursal: row.get(30),
        nom_sucursal: row.get(31),
        departamento: row.get(32),
        provincia: row.get(33),
        distrito: row.get(34),
        id_vendedor: row.get(35),
        nom_vendedor: row.get(36),
        id_pedido: row.get(37),
        ord_compra: row.try_get(38).unwrap_or_default(),
        file_source: row.get(39),
        mes_ref: row.get(40),
        tipo_operacion: row.try_get(41).unwrap_or_default(),
        factura_ref_serie: row.try_get(42).unwrap_or_default(),
        factura_ref_nro: row.try_get(43).unwrap_or_default(),
        folio_unico: row.try_get(44).unwrap_or_default(),
        id_ubigeo: row.try_get(45).unwrap_or_default(),
        canal_distribucion: row.try_get(46).unwrap_or_default(),
        nom_condicion_pago: row.try_get(47).unwrap_or_default(),
        estado_linea: row.try_get(48).unwrap_or_default(),
        division: row.try_get(49).unwrap_or_default(),
        fec_cargo: row.try_get(50).unwrap_or_default(),
        id_guia: row.try_get(51).unwrap_or_default(),
    };
    (row.get(0), v)
}

