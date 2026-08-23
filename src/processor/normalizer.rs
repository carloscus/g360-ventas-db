use crate::models::Venta;

pub fn normalize_ids(ventas: &mut [Venta]) {
    for v in ventas {
        v.id_articulo = lz(&v.id_articulo, 6);
        v.id_cliente = lz(&v.id_cliente, 5);
        v.id_linea = lz(&v.id_linea, 4);
        v.id_vendedor = lz(&v.id_vendedor, 5);
        v.serie_doc = lz(&v.serie_doc, 3);
    }
}

fn lz(s: &str, w: usize) -> String {
    let s = s.trim().replace(".", "");
    if s.is_empty() {
        return String::new();
    }
    format!("{:0>width$}", s, width = w)
}

pub fn normalize_text(ventas: &mut [Venta]) {
    for v in ventas {
        v.nom_articulo = v.nom_articulo.trim().to_string();
        v.nom_cliente = v.nom_cliente.trim().to_string();
        v.nom_linea = v.nom_linea.trim().to_string();
        v.tpo_doc = v.tpo_doc.to_uppercase();
    }
}
