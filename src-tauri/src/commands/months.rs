/// Gate de integridad para importacion manual: rechaza XLS cuya columna SKU
/// (ID_ARTICULO) este guardada como numero. En ese caso Excel pudo borrar los
/// ceros a la izquierda (011019 -> 11019) y la importacion crearia SKUs falsos
/// que colisionan con productos de otra linea. La captura automatica descarga
/// el XLS original del intranet (texto), por eso solo se valida al importar a mano.
pub fn reject_numeric_sku(xls_path: &str) -> Result<(), String> {
    match g360_db_ventas::processor::xls::inspect_sku_column(std::path::Path::new(xls_path)) {
        Ok(insp) if insp.has_numeric_sku() => {
            let n = insp.numeric_cells;
            let total = insp.total_cells;
            Err(format!(
                "⛔ Importación bloqueada: la columna SKU está guardada como número en {n} de {total} filas. \
                 Excel pudo eliminar ceros a la izquierda (011019 → 11019) y crear SKUs falsos. \
                 Vuelve a exportar/descargar el XLS del intranet SIN abrirlo ni re-guardarlo en Excel. \
                 Detalle: {}",
                insp.warning_message()
            ))
        }
        Ok(_) => Ok(()),
        Err(e) => {
            // Si no se pudo inspeccionar (formato raro), no bloquear: el parseo
            // posterior validara el contenido. Solo se registra.
            eprintln!("inspect_sku_column no concluyente para {}: {}", xls_path, e);
            Ok(())
        }
    }
}
