// Comentarios en español - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use crate::models::Venta;
use anyhow::Result;
use std::path::Path;
use tracing::info;

fn parse_f64(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    if s.contains('.') && s.contains(',') {
        s.replace('.', "")
            .replace(',', ".")
            .parse::<f64>()
            .unwrap_or(0.0)
    } else if s.contains(',') {
        s.replace(',', ".").parse::<f64>().unwrap_or(0.0)
    } else {
        s.parse::<f64>().unwrap_or(0.0)
    }
}

fn clean_str(s: &str) -> String {
    s.trim()
        .replace('\u{00A0}', " ")
        .replace("  ", " ")
        .chars()
        .map(|c| match c {
            '\u{00C1}' => 'A',
            '\u{00C9}' => 'E',
            '\u{00CD}' => 'I',
            '\u{00D3}' => 'O',
            '\u{00DA}' => 'U',
            '\u{00D1}' => 'N',
            '\u{00E1}' => 'a',
            '\u{00E9}' => 'e',
            '\u{00ED}' => 'i',
            '\u{00F3}' => 'o',
            '\u{00FA}' => 'u',
            '\u{00F1}' => 'n',
            _ => c,
        })
        .collect()
}

fn clean_id(s: &str) -> String {
    let mut t = s.trim().replace('\u{feff}', "").trim().to_string();
    if t.is_empty() {
        return String::new();
    }
    let low = t.to_lowercase();
    if low == "nan" || low == "none" || low == "null" || low == "<na>" {
        return String::new();
    }
    loop {
        if t.ends_with(".0") {
            let mut cut = t.len();
            while cut > 0 && t.as_bytes()[cut - 1] == b'0' {
                cut -= 1;
            }
            if cut > 0 && t.as_bytes()[cut - 1] == b'.' {
                t.truncate(cut - 1);
                continue;
            }
        }
        break;
    }
    t
}
fn normalize_doc_id(s: &str) -> String {
    clean_id(s)
}
fn normalize_client_id(s: &str) -> String {
    let t = clean_id(s);
    if t.is_empty() {
        return String::new();
    }
    if t.len() <= 8 {
        format!("{:0>8}", t)
    } else {
        format!("{:0>11}", t)
    }
}

fn parse_date(s: &str) -> chrono::NaiveDate {
    let s = s.trim();
    if s.is_empty() {
        return chrono::NaiveDate::default();
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%d/%m/%Y") {
        return d;
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%d-%m-%Y") {
        return d;
    }
    chrono::NaiveDate::default()
}

/// Parsea formato completo 44 columnas
pub fn parse_export_csv(path: &Path) -> Result<Vec<Venta>> {
    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .replace("ventas_", "");
    let mut reader = csv::Reader::from_path(path)?;
    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|h| h.trim().to_uppercase())
        .collect();
    let col = |name: &str| -> Option<usize> { headers.iter().position(|h| h.contains(name)) };
    let c_anho = col("ANHO");
    let _c_mes = col("MES");
    let c_ic = col("ID_CLIENTE");
    let c_dc = col("DOC_CLIENTE");
    let c_nc = col("NOM_CLIENTE");
    let c_dep = col("NOM_DEPARTAMENTO");
    let c_pro = col("NOM_PROVINCIA");
    let c_dis = col("NOM_DISTRITO");
    let c_il = col("ID_LINEA");
    let c_nl = col("NOM_LINEA");
    let c_ig = col("ID_GRUPO");
    let c_ng = col("NOM_GRUPO");
    let c_it = col("ID_TIPO");
    let c_nt = col("NOM_TIPO");
    let c_if = col("ID_FAMILIA");
    let c_nf = col("NOM_FAMILIA");
    let c_el = col("ESTADO_LINEA");
    let c_ia = col("ID_ARTICULO");
    let c_na = col("NOM_ARTICULO");
    let c_iv = col("ID_VENDEDOR");
    let c_nv = col("NOM_VENDEDOR");
    let c_cd = col("CANAL DE DISTRIBUCION");
    let c_cs = col("COD_SUCURSAL");
    let c_ns = col("NOM_SUCURSAL");
    let c_td = col("TPO_DOC");
    let c_sd = col("SERIE_DOC");
    let c_nd = col("NRO_DOC");
    let _c_oc = col("ORD_COMPRA");
    let _c_gu = col("ID_GUIA");
    let c_fo = col("FECHA_ORIG");
    let c_ref = col("REFERENCIA");
    let c_fr = col("FECHA_REF");
    let c_mn = col("MONEDA");
    let c_qt = col("CANTIDAD");
    let c_sl = col("SOLES").or(col("TOTAL"));
    let c_do = col("DOLARES");
    let _c_cp = col("NOM_CONDICION_PAGO");
    let c_ip = col("ID_PEDIDO");
    let c_fv = col("FECHA_VENC");
    let _c_div = col("DIVISION");
    let c_fc = col("FEC_CARGO");
    let mut out = Vec::new();
    for row in reader.records() {
        let r = row?;
        let g = |i: Option<usize>| -> String { clean_str(i.and_then(|x| r.get(x)).unwrap_or("")) };
        let cantidad = parse_f64(&g(c_qt));
        let soles_raw = parse_f64(&g(c_sl));
        let soles = (soles_raw * 100.0).round() / 100.0;
        let dolares_raw = parse_f64(&g(c_do));
        let dolares = (dolares_raw * 100.0).round() / 100.0;
        let precio_raw = if cantidad != 0.0 {
            soles / cantidad
        } else {
            0.0
        };
        let precio_unitario = (precio_raw * 1_000_000.0).round() / 1_000_000.0;
        let v = Venta {
            id_articulo: normalize_doc_id(&g(c_ia)),
            original_sku: g(c_ia),
            nom_articulo: g(c_na),
            id_linea: g(c_il),
            nom_linea: g(c_nl),
            id_grupo: g(c_ig),
            nom_grupo: g(c_ng),
            id_tipo: g(c_it),
            nom_tipo: g(c_nt),
            id_familia: g(c_if),
            nom_familia: g(c_nf),
            estado_linea: g(c_el),
            id_cliente: normalize_client_id(&g(c_ic)),
            doc_cliente: g(c_dc),
            nom_cliente: g(c_nc),
            tpo_doc: g(c_td).to_uppercase(),
            serie_doc: g(c_sd),
            nro_doc: g(c_nd),
            referencia: g(c_ref),
            moneda: {
                let m = g(c_mn);
                if m.is_empty() {
                    "Soles".to_string()
                } else {
                    m
                }
            },
            cantidad,
            soles,
            dolares,
            precio_unitario,
            anho: g(c_anho).parse().unwrap_or(0),
            mes: 0,
            fecha_orig: parse_date(&g(c_fo)),
            fecha_ref: {
                let s = g(c_fr);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            },
            fecha_venc: {
                let s = g(c_fv);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            },
            fec_cargo: {
                let s = g(c_fc);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            },
            cod_sucursal: g(c_cs),
            nom_sucursal: g(c_ns),
            departamento: g(c_dep),
            provincia: g(c_pro),
            distrito: g(c_dis),
            canal_dist: g(c_cd),
            id_vendedor: g(c_iv),
            nom_vendedor: g(c_nv),
            id_pedido: g(c_ip),
            file_source: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            mes_ref: label.clone(),
        };
        if !v.id_articulo.is_empty()
            && !v.id_cliente.is_empty()
            && crate::config::is_allowed_line(&v.id_linea)
        {
            out.push(v);
        }
    }
    info!("parse_export: {} rows from {}", out.len(), path.display());
    Ok(out)
}

/// Parsea formato simple 7 columnas
pub fn parse_simple_csv(path: &Path) -> Result<Vec<Venta>> {
    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .replace("ventas_", "");
    let mut reader = csv::Reader::from_path(path)?;
    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|h| h.trim().to_uppercase())
        .collect();
    let col = |name: &str| -> Option<usize> { headers.iter().position(|h| h.contains(name)) };
    let nro_doc = col("NRO_DOCUMENTO");
    let nom_cliente = col("NOM_CLIENTE");
    let cod_sucursal = col("COD_SUCURSAL");
    let nom_sucursal = col("NOM_SUCURSAL");
    let cantidad = col("CANTIDAD");
    let soles = col("SOLES");
    let dolares = col("DOLARES");
    let mut out = Vec::new();
    for row in reader.records() {
        let r = row?;
        let g = |i: Option<usize>| -> String { clean_str(i.and_then(|x| r.get(x)).unwrap_or("")) };
        let doc = g(nro_doc);
        if doc.is_empty() || doc == "NRO_DOCUMENTO" {
            continue;
        }
        let v = Venta {
            id_articulo: String::new(),
            original_sku: String::new(),
            nom_articulo: String::new(),
            id_linea: String::new(),
            nom_linea: String::new(),
            id_grupo: String::new(),
            nom_grupo: String::new(),
            id_tipo: String::new(),
            nom_tipo: String::new(),
            id_familia: String::new(),
            nom_familia: String::new(),
            estado_linea: String::new(),
            id_cliente: normalize_client_id(&doc),
            doc_cliente: String::new(),
            nom_cliente: g(nom_cliente),
            tpo_doc: String::new(),
            serie_doc: String::new(),
            nro_doc: doc.clone(),
            referencia: String::new(),
            moneda: "Soles".to_string(),
            cantidad: g(cantidad)
                .replace(".", "")
                .replace(",", ".")
                .parse()
                .unwrap_or(0.0),
            soles: g(soles)
                .replace(".", "")
                .replace(",", ".")
                .parse()
                .unwrap_or(0.0),
            dolares: g(dolares)
                .replace(".", "")
                .replace(",", ".")
                .parse()
                .unwrap_or(0.0),
            precio_unitario: 0.0,
            anho: 0,
            mes: 0,
            fecha_orig: chrono::NaiveDate::default(),
            fecha_ref: None,
            fecha_venc: None,
            fec_cargo: None,
            cod_sucursal: g(cod_sucursal),
            nom_sucursal: g(nom_sucursal),
            departamento: String::new(),
            provincia: String::new(),
            distrito: String::new(),
            canal_dist: String::new(),
            id_vendedor: String::new(),
            nom_vendedor: String::new(),
            id_pedido: String::new(),
            file_source: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            mes_ref: label.clone(),
        };
        if !v.id_cliente.is_empty() {
            out.push(v);
        }
    }
    info!("parse_simple: {} rows from {}", out.len(), path.display());
    Ok(out)
}
