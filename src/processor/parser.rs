// Comentarios en español - fechas internas yyyy-mm-dd, display dd/mm/yyyy
use crate::models::Venta;
use anyhow::Result;
use std::path::Path;
use tracing::info;

/// Parsea un valor numerico con contexto de columna.
/// is_quantity=true: trata '.' como decimal SIEMPRE (nunca miles).
/// is_quantity=false: detecta formato segun heuristica (miles o decimal).
fn parse_f64(s: &str, is_quantity: bool) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    // Columnas de cantidad: punto siempre es decimal, coma se elimina
    // Evita que "1.000" o "25.056" se confundan con miles
    if is_quantity {
        return s.replace(',', "").parse::<f64>().unwrap_or(0.0);
    }
    // Para montos: detectar formato segun contexto
    if s.contains('.') && s.contains(',') {
        // "1.000,50" -> Latin format: period=thousand, comma=decimal
        s.replace('.', "")
            .replace(',', ".")
            .parse::<f64>()
            .unwrap_or(0.0)
    } else if s.contains(',') {
        // Heuristica: 3+ digitos despues de la coma => separador de miles
        if let Some(pos) = s.find(',') {
            let decimals = s.len() - pos - 1;
            if decimals >= 3 {
                s.replace(',', "")
                    .parse::<f64>()
                    .unwrap_or(0.0)
            } else {
                s.replace(',', ".")
                    .parse::<f64>()
                    .unwrap_or(0.0)
            }
        } else {
            0.0
        }
    } else if s.contains('.') {
        // Heuristica: "1.000" (3 decimales todos cero) => miles
        // "25.056" (3 decimales NO todos cero) => decimal real
        if let Some(pos) = s.find('.') {
            let after = &s[pos+1..];
            if after.len() == 3 && after == "000" {
                s.replace('.', "")
                    .parse::<f64>()
                    .unwrap_or(0.0)
            } else {
                s.parse::<f64>().unwrap_or(0.0)
            }
        } else {
            0.0
        }
    } else {
        s.parse::<f64>().unwrap_or(0.0)
    }
}

/// Wrapper que determina is_quantity desde el nombre de la columna.
/// Columnas de cantidad/ unidades => is_quantity=true.
/// Columnas de monto/ precio => is_quantity=false.
fn parse_f64_ctx(s: &str, col_name: &str) -> f64 {
    let up = col_name.trim().to_uppercase();
    let is_quantity = up == "CANTIDAD"
        || up == "CANTIDAD FAE"
        || up.contains("UNIDADES")
        || up.contains("PRECIO UNITARIO");
    parse_f64(s, is_quantity)
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

/// Computa tipo_operacion + factura_ref (serie/nro) derivados del documento.
/// referencia formato: "F01/204-50867" -> serie=204 nro=50867
pub fn derivar_campos(v: &mut crate::models::Venta) {
    let tpo = v.tpo_doc.as_str();
    let cant = v.cantidad;
    let has_ref = !v.referencia.trim().is_empty();
    v.tipo_operacion = match tpo {
        "NCR" if cant < 0.0 => "devolucion".to_string(),
        "NCR" => "ajuste_valor".to_string(),
        "NDB" => "nota_debito".to_string(),
        _ => "venta".to_string(),
    };
    if has_ref {
        // "F01/204-50867" -> parte[0]=F01 parte[1]=204-50867
        let rest = v.referencia.rsplit('/').next().unwrap_or("").to_string();
        if let Some((s, n)) = rest.split_once('-') {
            v.factura_ref_serie = normalize_doc_id(s).to_uppercase();
            v.factura_ref_nro = normalize_doc_id(n);
        }
    }
    // folio_unico: ID unico para lookups y dedup en CRM
    v.folio_unico = format!("{}/{}/{}", v.tpo_doc, v.serie_doc, v.nro_doc);
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

/// NCR/NDB -> es nota de credito/debito (valida por factura referenciada)
fn es_nc_nd(tpo_doc: &str) -> bool {
    let t = tpo_doc.to_uppercase();
    t.contains("NCR") || t.contains("NDB")
}

/// Resultado del parse con NC/ND pendientes de validar contra la BD (cross-mes).
pub struct ParseOutput {
    pub ventas: Vec<Venta>,
    /// NC/ND cuya factura referenciada NO esta en el mismo archivo (requieren lookup BD)
    pub nc_nd_pendientes: Vec<Venta>,
}

/// Core de parseo compartido: lee el CSV y aplica filtros.
/// - ventas: filas con linea permitida + NC/ND cuya factura ref (en-archivo) pasa allowlist
/// - nc_nd_pendientes: NC/ND con REFERENCIA F01/serie-nro pero factura no presente en el archivo
fn parse_export_csv_inner(path: &Path) -> Result<ParseOutput> {
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
    let c_ia = col("ID_ARTICULO");
    let c_na = col("NOM_ARTICULO");
    let c_iv = col("ID_VENDEDOR");
    let c_nv = col("NOM_VENDEDOR");
    let _c_cd = col("CANAL DE DISTRIBUCION");
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
    let c_fae = col("CANTIDAD FAE");
    let c_sl = col("SOLES").or(col("TOTAL"));
    let c_do = col("DOLARES");
    let _c_cp = col("NOM_CONDICION_PAGO");
    let c_ip = col("ID_PEDIDO");
    let c_fv = col("FECHA_VENC");
    let _c_div = col("DIVISION");
    let mut raw_rows: Vec<Venta> = Vec::new();
    for row in reader.records() {
        let r = row?;
        let g = |i: Option<usize>| -> String { clean_str(i.and_then(|x| r.get(x)).unwrap_or("")) };
        let cantidad = parse_f64_ctx(&g(c_qt), "CANTIDAD");
        let cantidad_fae = parse_f64_ctx(&g(c_fae), "CANTIDAD FAE");
        let soles_raw = parse_f64_ctx(&g(c_sl), "SOLES");
        let soles = (soles_raw * 100.0).round() / 100.0;
        let dolares_raw = parse_f64_ctx(&g(c_do), "DOLARES");
        let dolares = (dolares_raw * 100.0).round() / 100.0;
        // Base del precio unitario: preferir cantidad real; si es 0 (ajuste de valor/descuento),
        // usar CANTIDAD FAE (base sobre la que se aplico el descuento).
        // Ej. vendi 1000 pero descuente 500 entregados => precio aplicado = soles / FAE.
        let base_qty = if cantidad != 0.0 { cantidad } else { cantidad_fae };
        let precio_raw = if base_qty != 0.0 {
            soles / base_qty
        } else {
            0.0
        };
        let precio_unitario = (precio_raw * 10_000.0).round() / 10_000.0;
        let mut v = Venta {
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
            id_cliente: normalize_client_id(&g(c_ic)),
            doc_cliente: g(c_dc),
            nom_cliente: g(c_nc),
            tpo_doc: g(c_td).to_uppercase(),
            serie_doc: normalize_doc_id(&g(c_sd)).to_uppercase(),
            nro_doc: normalize_doc_id(&g(c_nd)),
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
            cantidad_fae,
            soles,
            dolares,
            precio_unitario,
            anho: g(c_anho).parse().unwrap_or(0),
            mes: label.get(5..7).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0),
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
            cod_sucursal: g(c_cs),
            nom_sucursal: g(c_ns),
            departamento: g(c_dep),
            provincia: g(c_pro),
            distrito: g(c_dis),
            id_vendedor: g(c_iv),
            nom_vendedor: g(c_nv),
            id_pedido: g(c_ip),
            file_source: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            mes_ref: label.clone(),
            tipo_operacion: String::new(),
            factura_ref_serie: String::new(),
            factura_ref_nro: String::new(),
            folio_unico: String::new(),
        };
        derivar_campos(&mut v);
        if !v.id_articulo.is_empty() && !v.id_cliente.is_empty() {
            raw_rows.push(v);
        }
    }
    // Indice de facturas del archivo: (serie,nro) -> alguna linea permitida?
    // Solo facturas de venta/compra (F01/BDI/FEX/FDI/01B...), no NC/ND
    let mut facturas: std::collections::HashMap<(String, String), bool> = std::collections::HashMap::new();
    for v in &raw_rows {
        if !es_nc_nd(&v.tpo_doc) && !v.serie_doc.is_empty() && !v.nro_doc.is_empty() {
            let ok = crate::config::is_allowed_line(&v.id_linea);
            facturas
                .entry((v.serie_doc.clone(), v.nro_doc.clone()))
                .and_modify(|e| *e = *e || ok)
                .or_insert(ok);
        }
    }
    let mut out = Vec::new();
    let mut nc_nd_pendientes = Vec::new();
    for v in raw_rows {
        if crate::config::is_allowed_line(&v.id_linea) {
            out.push(v);
            continue;
        }
        // NC/ND con linea no permitida: validar contra la FACTURA REFERENCIADA
        if es_nc_nd(&v.tpo_doc) {
            // REFERENCIA formato "F01/201-17137" -> serie=201 nro=17137 (ya parseado en factura_ref_*)
            let (serie, nro) = (v.factura_ref_serie.clone(), v.factura_ref_nro.clone());
            if serie.is_empty() || nro.is_empty() {
                continue; // NC sin factura ref clara -> se excluye
            }
            if let Some(ok) = facturas.get(&(serie.clone(), nro.clone())) {
                if *ok {
                    out.push(v);
                }
            } else {
                // Factura de otro mes -> requiere lookup contra la BD (cross-mes)
                nc_nd_pendientes.push(v);
            }
        }
        // Otras lineas no permitidas (traslados/servicios fuera del allowlist): se excluyen
    }
    info!("parse_export: {} rows from {}", out.len(), path.display());
    Ok(ParseOutput { ventas: out, nc_nd_pendientes })
}

/// API compatible: parsea sin lookup cross-mes (NC/ND con factura en otro mes se excluyen).
pub fn parse_export_csv(path: &Path) -> Result<Vec<Venta>> {
    Ok(parse_export_csv_inner(path)?.ventas)
}

/// Parsea + resuelve NC/ND cross-mes contra la BD. Para el flujo principal de captura.
pub async fn parse_export_csv_with_cross(path: &Path, pool: &sqlx::SqlitePool) -> Result<Vec<Venta>> {
    let mut output = parse_export_csv_inner(path)?;
    if !output.nc_nd_pendientes.is_empty() {
        info!("  cross-month: {} NC/ND pendientes, resolviendo...", output.nc_nd_pendientes.len());
        resolve_nc_nd_cross(pool, &mut output.nc_nd_pendientes).await;
        // Las resueltas se agregan al resultado final
        output.ventas.append(&mut output.nc_nd_pendientes);
    }
    Ok(output.ventas)
}

/// Resuelve NC/ND pendientes (su factura no esta en el mismo CSV) contra la BD.
/// Si la factura referenciada existe en la BD con linea permitida, se incluye.
pub async fn resolve_nc_nd_cross(pool: &sqlx::SqlitePool, pendientes: &mut Vec<Venta>) {
    if pendientes.is_empty() {
        return;
    }
    let mut resueltas = Vec::new();
    let mut sin_resolver = Vec::new();
    for v in pendientes.drain(..) {
        let serie = &v.factura_ref_serie;
        let nro = &v.factura_ref_nro;
        if serie.is_empty() || nro.is_empty() {
            sin_resolver.push(v);
            continue;
        }
        // Buscar la factura referenciada en la BD
        let exists: bool = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ventas WHERE serie_doc = ? AND nro_doc = ? AND (tpo_doc LIKE 'F01%' OR tpo_doc = 'F01')"
        )
        .bind(serie)
        .bind(nro)
        .fetch_one(pool)
        .await
        .unwrap_or(0) > 0;
        if exists {
            resueltas.push(v);
        } else {
            sin_resolver.push(v);
        }
    }
    let n_res = resueltas.len();
    let n_sin = sin_resolver.len();
    if n_res > 0 {
        info!("  cross-month: {n_res} NC/ND resueltas, {n_sin} sin factura en BD");
    }
    // Agregar las resueltas al resultado final
    pendientes.extend(resueltas);
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
        let mut v = Venta {
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
            id_cliente: normalize_client_id(&doc),
            doc_cliente: String::new(),
            nom_cliente: g(nom_cliente),
            tpo_doc: String::new(),
            serie_doc: String::new(),
            nro_doc: doc.clone(),
            referencia: String::new(),
            moneda: "Soles".to_string(),
            cantidad: parse_f64_ctx(&g(cantidad), "CANTIDAD"),
            cantidad_fae: 0.0,
            soles: parse_f64_ctx(&g(soles), "SOLES"),
            dolares: parse_f64_ctx(&g(dolares), "DOLARES"),
            precio_unitario: 0.0,
            anho: label.get(0..4).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0),
            mes: label.get(5..7).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0),
            fecha_orig: chrono::NaiveDate::default(),
            fecha_ref: None,
            fecha_venc: None,
            cod_sucursal: g(cod_sucursal),
            nom_sucursal: g(nom_sucursal),
            departamento: String::new(),
            provincia: String::new(),
            distrito: String::new(),
            id_vendedor: String::new(),
            nom_vendedor: String::new(),
            id_pedido: String::new(),
            file_source: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            mes_ref: label.clone(),
            tipo_operacion: String::new(),
            factura_ref_serie: String::new(),
            factura_ref_nro: String::new(),
            folio_unico: String::new(),
        };
        derivar_campos(&mut v);
        if !v.id_cliente.is_empty() {
            out.push(v);
        }
    }
    info!("parse_simple: {} rows from {}", out.len(), path.display());
    Ok(out)
}
