// Parser CSV con normalización completa para cartuchos offline.
// Todas las transformaciones se hacen AL INSERTAR para que nunca haya
// datos sucios en la DB local.

use crate::models::Venta;
use anyhow::{anyhow, Result};
use chrono::Datelike;
use std::path::Path;
use tracing::info;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers de parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Parsea un valor numerico con contexto de columna.
/// is_quantity=true: trata '.' como decimal SIEMPRE (nunca miles); conserva
///   decimales (redondeo a 4 solo para limpiar ruido float).
/// is_quantity=false: detecta formato segun heuristica (miles o decimal).
fn parse_f64(s: &str, is_quantity: bool) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    if is_quantity {
        let v = s.replace(',', "").parse::<f64>().unwrap_or(0.0);
        return (v * 10_000.0).round() / 10_000.0;
    }
    if s.contains('.') && s.contains(',') {
        s.replace('.', "").replace(',', ".").parse::<f64>().unwrap_or(0.0)
    } else if s.contains(',') {
        if let Some(pos) = s.find(',') {
            let decimals = s.len() - pos - 1;
            if decimals >= 3 {
                s.replace(',', "").parse::<f64>().unwrap_or(0.0)
            } else {
                s.replace(',', ".").parse::<f64>().unwrap_or(0.0)
            }
        } else {
            0.0
        }
    } else if s.contains('.') {
        if let Some(pos) = s.find('.') {
            let after = &s[pos+1..];
            if after.len() == 3 && after == "000" {
                s.replace('.', "").parse::<f64>().unwrap_or(0.0)
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

/// ID_LINEA canonico: 2 chars sin prefijo (MA, 01, 78...).
fn normalize_line_id(s: &str) -> String {
    let t = clean_id(s).to_uppercase();
    if t.starts_with("01") && t.len() >= 3 {
        t[2..].to_string()
    } else if t.len() == 4 && t.starts_with("01") {
        t[2..].to_string()
    } else if t.len() == 2 {
        t
    } else {
        t
    }
}

/// ID_VENDEDOR canonico: 3 chars sin prefijo (177, A02, PE1...).
/// La fuente trae '01'+3 chars ('01052' -> '052', '01M05' -> 'M05'), pero
/// hay anomalias de 4 chars ('0154') que sin re-padear quedarian en '54':
/// si el resultado es numerico y corto, se rellena a 3 ('054').
fn normalize_seller_id(s: &str) -> String {
    let t = clean_id(s).to_uppercase();
    let stripped = if t.starts_with("01") && t.len() >= 4 {
        t[2..].to_string()
    } else {
        t
    };
    if stripped.len() < 3 && !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
        format!("{:0>3}", stripped)
    } else {
        stripped
    }
}

/// Calcula id_pedido normalizado: "SADIE 25" -> "SADIE_25"
fn normalize_id_pedido(s: &str) -> String {
    let raw = s.trim();
    if raw.is_empty() {
        return String::new();
    }
    raw.split_whitespace().collect::<Vec<_>>().join("_")
}

/// Normaliza un ID de entrada del usuario para búsquedas en la BD.
///
/// La BD almacena IDs ya normalizados (zfill para clientes, sin prefijo '01'
/// para líneas/vendedores). Si el usuario escribe "10214", esta función la
/// convierte en "00010214" para que el WHERE la encuentre.
///
/// Uso en la app cliente:
/// ```rust
/// use g360_db_ventas::processor::parser::{normalize_query_id, IdType};
/// let search = normalize_query_id("10214", IdType::Cliente);
/// assert_eq!(search, "00010214");
/// ```
pub fn normalize_query_id(input: &str, id_type: IdType) -> String {
    let t = clean_id(input);
    if t.is_empty() {
        return String::new();
    }
    match id_type {
        IdType::Cliente => normalize_client_id(&t),
        IdType::Linea => normalize_line_id(&t),
        IdType::Vendedor => normalize_seller_id(&t),
        IdType::Articulo => t.to_uppercase(),
    }
}

/// Tipo de ID para normalización de queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdType {
    Cliente,
    Linea,
    Vendedor,
    Articulo,
}

/// Parsea fecha en ISO (yyyy-mm-dd) desde varios formatos.
/// Retorna None si no es parseable o es fecha futura.
fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%d/%m/%Y") {
        return Some(d);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%d-%m-%Y") {
        return Some(d);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Funcion principal de parseo
// ─────────────────────────────────────────────────────────────────────────────

/// Parsea un CSV exportado del ERP y normaliza TODAS las columnas críticas.
/// El filtro de lineas (allowlist) NO se aplica aqui: la DB local guarda el
/// export completo del ERP (raw-first). El filtro vive en el upload a Supabase.
///
/// Normalizaciones aplicadas:
/// - id_pedido: espacios -> guion_bajo
/// - mes_ref, anho, mes: derivados de fecha_orig (NO del nombre del archivo)
/// - fecha_orig: validada contra fecha futura
/// - id_linea, id_vendedor: normalizados a formato canonico
/// - id_cliente: normalizado a 8/11 digitos
///
/// Retorna estadisticas de calidad para incluir en el manifest del cartucho.
pub fn parse_export_csv(path: &Path) -> Result<(Vec<Venta>, QualityStats)> {
    let _label = path
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

    let _c_anho = col("ANHO");
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
    let c_oc = col("ORD_COMPRA");
    let c_ug = col("ID_GUIA");
    let c_fo = col("FECHA_ORIG");
    let c_ref = col("REFERENCIA");
    let c_fr = col("FECHA_REF");
    let c_mn = col("MONEDA");
    let c_qt = col("CANTIDAD");
    let c_fae = col("CANTIDAD FAE");
    let c_sl = col("SOLES").or(col("TOTAL"));
    let c_do = col("DOLARES");
    let _c_div = col("DIVISION");
    let c_div = col("DIVISION");
    let c_ube = col("ID_LOCALIDAD_UBIGEO").or(col("UBIGEO"));
    let c_canal = col("CANAL DE DISTRIBUCION").or(col("CANAL_DISTRIBUCION"));
    let c_cpg = col("NOM_CONDICION_PAGO").or(col("CONDICION_PAGO"));
    let c_est = col("ESTADO_LINEA");
    let c_fec = col("FEC_CARGO");
    let c_ip = col("ID_PEDIDO");
    let c_fv = col("FECHA_VENC");

    let required = [
        "ID_ARTICULO", "ID_CLIENTE", "FECHA_ORIG", "TPO_DOC", "SERIE_DOC", "NRO_DOC",
    ];
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|n| !headers.iter().any(|h| h.contains(n)))
        .collect();
    if !missing.is_empty() {
        return Err(anyhow!(
            "Headers requeridos faltantes en {}: {}. El export del intranet cambió de formato.",
            path.display(),
            missing.join(", ")
        ));
    }

    let today = chrono::Utc::now().date_naive();
    let mut raw_rows: Vec<Venta> = Vec::new();
    let mut stats = QualityStats::default();

    for row in reader.records() {
        let r = row?;
        let g = |i: Option<usize>| -> String { clean_str(i.and_then(|x| r.get(x)).unwrap_or("")) };

        let fecha_orig_raw = g(c_fo);
        let Some(fecha_orig) = parse_date(&fecha_orig_raw) else {
            stats.invalid_dates += 1;
            continue;
        };

        // RECHAZAR FECHAS FUTURAS: protege contra archivos nombrados mal o datos corruptos.
        if fecha_orig > today {
            stats.future_dates += 1;
            continue;
        }

        let cantidad = parse_f64_ctx(&g(c_qt), "CANTIDAD");
        let cantidad_fae = parse_f64_ctx(&g(c_fae), "CANTIDAD FAE");
        let soles_raw = parse_f64_ctx(&g(c_sl), "SOLES");
        let soles = (soles_raw * 100.0).round() / 100.0;
        let dolares_raw = parse_f64_ctx(&g(c_do), "DOLARES");
        let dolares = (dolares_raw * 100.0).round() / 100.0;

        // Base del precio unitario: preferir cantidad real; si es 0 (ajuste de valor/descuento),
        // usar CANTIDAD FAE (base sobre la que se aplico el descuento).
        let base_qty = if cantidad != 0.0 { cantidad } else { cantidad_fae };
        let precio_raw = if base_qty != 0.0 { soles / base_qty } else { 0.0 };
        let precio_unitario = (precio_raw * 100_000.0).round() / 100_000.0;

        // MES_REF, ANHO, MES derivados de fecha_orig (NO del nombre del archivo).
        let mes_ref = fecha_orig.format("%Y-%m").to_string();
        let anho = fecha_orig.year() as i32;
        let mes = fecha_orig.month() as i32;

        // ID_PEDIDO normalizado: espacios -> guion_bajo
        let id_pedido = normalize_id_pedido(&g(c_ip));

        let mut v = Venta {
            id_articulo: clean_id(&g(c_ia)),
            original_sku: g(c_ia),
            nom_articulo: g(c_na),
            id_linea: normalize_line_id(&g(c_il)),
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
            serie_doc: clean_id(&g(c_sd)).to_uppercase(),
            nro_doc: clean_id(&g(c_nd)),
            referencia: g(c_ref),
            moneda: {
                let m = g(c_mn).to_uppercase();
                match m.as_str() {
                    "" | "SOL" | "SOLES" | "PEN" | "S/" => "Soles".to_string(),
                    "DOL" | "DOLARES" | "USD" | "US$" | "$" => "Dolares".to_string(),
                    _ => m,
                }
            },
            cantidad,
            cantidad_fae,
            soles,
            dolares,
            precio_unitario,
            anho,
            mes,
            fecha_orig,
            fecha_ref: {
                let s = g(c_fr);
                if s.is_empty() { None } else { Some(parse_date(&s).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or(s)) }
            },
            fecha_venc: {
                let s = g(c_fv);
                if s.is_empty() { None } else { Some(parse_date(&s).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or(s)) }
            },
            cod_sucursal: g(c_cs),
            nom_sucursal: g(c_ns),
            departamento: g(c_dep),
            provincia: g(c_pro),
            distrito: g(c_dis),
            id_vendedor: normalize_seller_id(&g(c_iv)),
            nom_vendedor: g(c_nv),
            id_pedido,
            ord_compra: g(c_oc),
            file_source: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
            mes_ref,
            tipo_operacion: String::new(),
            factura_ref_serie: String::new(),
            factura_ref_nro: String::new(),
            folio_unico: String::new(),
            id_ubigeo: g(c_ube),
            canal_distribucion: g(c_canal),
            nom_condicion_pago: g(c_cpg),
            estado_linea: g(c_est),
            division: g(c_div),
            fec_cargo: {
                let s = g(c_fec);
                if s.is_empty() {
                    String::new()
                } else {
                    parse_date(&s).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or(s)
                }
            },
            id_guia: g(c_ug),
        };
        derivar_campos(&mut v);

        if !v.id_articulo.is_empty() && !v.id_cliente.is_empty() {
            raw_rows.push(v);
        } else {
            stats.empty_ids += 1;
        }
    }

    stats.total_rows = raw_rows.len() as u64 + stats.invalid_dates + stats.future_dates + stats.empty_ids;
    info!(
        "Parseado: {} filas validas, {} invalidas ({} fechas, {} futuras, {} sin ID)",
        raw_rows.len(),
        stats.invalid_dates + stats.future_dates + stats.empty_ids,
        stats.invalid_dates,
        stats.future_dates,
        stats.empty_ids,
    );
    Ok((raw_rows, stats))
}

/// Wrapper async para compatibilidad con el pipeline existente.
pub async fn parse_export_csv_with_cross(path: &Path, _pool: &sqlx::SqlitePool) -> Result<Vec<Venta>> {
    let (rows, _) = parse_export_csv(path)?;
    Ok(rows)
}

fn parse_f64_ctx(s: &str, col_name: &str) -> f64 {
    let up = col_name.trim().to_uppercase();
    let is_quantity = up == "CANTIDAD"
        || up == "CANTIDAD FAE"
        || up.contains("UNIDADES")
        || up.contains("PRECIO UNITARIO");
    parse_f64(s, is_quantity)
}

/// Computa tipo_operacion + factura_ref (serie/nro) derivados del documento.
pub fn derivar_campos(v: &mut crate::models::Venta) {
    let tpo = v.tpo_doc.as_str();
    let cant = v.cantidad;
    let has_ref = !v.referencia.trim().is_empty();
    v.tipo_operacion = match tpo {
        "NCR" if cant < 0.0 => "devolucion".to_string(),
        "NCR" | "NDB" => "ajuste_valor".to_string(),
        _ => "venta".to_string(),
    };
    if has_ref {
        let rest = v.referencia.rsplit('/').next().unwrap_or("").to_string();
        if let Some((s, n)) = rest.split_once('-') {
            v.factura_ref_serie = clean_id(s).to_uppercase();
            v.factura_ref_nro = clean_id(n);
        }
    }
    v.folio_unico = format!("{}/{}/{}", v.tpo_doc, v.serie_doc, v.nro_doc);
}

/// Estadisticas de calidad para incluir en el manifest del cartucho.
#[derive(Debug, Default, Clone)]
pub struct QualityStats {
    pub total_rows: u64,
    pub invalid_dates: u64,
    pub future_dates: u64,
    pub empty_ids: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_query_id: Cliente (zfill 8) ──

    #[test]
    fn cliente_corto_se_completa_con_ceros() {
        assert_eq!(normalize_query_id("10214", IdType::Cliente), "00010214");
        assert_eq!(normalize_query_id("2035", IdType::Cliente), "00002035");
        assert_eq!(normalize_query_id("56101", IdType::Cliente), "00056101");
    }

    #[test]
    fn cliente_de_8_digitos_se_mantiene() {
        assert_eq!(normalize_query_id("00010214", IdType::Cliente), "00010214");
        assert_eq!(normalize_query_id("00056101", IdType::Cliente), "00056101");
    }

    #[test]
    fn cliente_ruc_11_digitos_se_mantiene() {
        assert_eq!(
            normalize_query_id("20481150125", IdType::Cliente),
            "20481150125"
        );
    }

    // ── normalize_query_id: Linea (lstrip 01) ──

    #[test]
    fn linea_con_prefijo_01_se_quita() {
        assert_eq!(normalize_query_id("0178", IdType::Linea), "78");
        assert_eq!(normalize_query_id("0185", IdType::Linea), "85");
        assert_eq!(normalize_query_id("0102", IdType::Linea), "02");
    }

    #[test]
    fn linea_sin_prefijo_se_mantiene() {
        assert_eq!(normalize_query_id("78", IdType::Linea), "78");
        assert_eq!(normalize_query_id("AD", IdType::Linea), "AD");
    }

    // ── normalize_query_id: Vendedor (lstrip 01) ──

    #[test]
    fn vendedor_con_prefijo_01_se_quita() {
        assert_eq!(normalize_query_id("01M05", IdType::Vendedor), "M05");
        assert_eq!(normalize_query_id("01177", IdType::Vendedor), "177");
    }

    #[test]
    fn vendedor_sin_prefijo_se_mantiene() {
        assert_eq!(normalize_query_id("052", IdType::Vendedor), "052");
        assert_eq!(normalize_query_id("177", IdType::Vendedor), "177");
        assert_eq!(normalize_query_id("M05", IdType::Vendedor), "M05");
    }

    #[test]
    fn vendedor_anomalia_de_fuente_se_rellena_a_3_chars() {
        // Fuente real '0154' (4 chars, ver raw 2010-10/2011-03): sin el pad
        // quedaria en '54'. El canonico son 3 chars: '01052'->'052' y
        // '01M05'->'M05' -> '0154' debe dar '054'.
        assert_eq!(normalize_query_id("0154", IdType::Vendedor), "054");
        assert_eq!(normalize_query_id("54", IdType::Vendedor), "054");
        assert_eq!(normalize_query_id("01054", IdType::Vendedor), "054");
        // No-numéricos: solo se quita el prefijo, sin inventar padding.
        assert_eq!(normalize_query_id("01M5", IdType::Vendedor), "M5");
    }

    // ── normalize_query_id: Articulo (sin transformar) ──

    #[test]
    fn articulo_se_mantiene_igual() {
        assert_eq!(normalize_query_id("011883", IdType::Articulo), "011883");
        assert_eq!(normalize_query_id("85007", IdType::Articulo), "85007");
    }

    // ── normalize_query_id: Edge cases ──

    #[test]
    fn input_vacio_devuelve_vacio() {
        assert_eq!(normalize_query_id("", IdType::Cliente), "");
        assert_eq!(normalize_query_id("  ", IdType::Linea), "");
    }

    // ── derivar_campos: clasificación NC/ND ──

    #[test]
    fn ncr_con_cantidad_negativa_es_devolucion() {
        let mut v = crate::models::Venta::nuevo("test.csv", "2026-11");
        v.tpo_doc = "NCR".to_string();
        v.cantidad = -10.0;
        derivar_campos(&mut v);
        assert_eq!(v.tipo_operacion, "devolucion");
    }

    #[test]
    fn ncr_con_cantidad_cero_es_ajuste_valor() {
        let mut v = crate::models::Venta::nuevo("test.csv", "2026-11");
        v.tpo_doc = "NCR".to_string();
        v.cantidad = 0.0;
        derivar_campos(&mut v);
        assert_eq!(v.tipo_operacion, "ajuste_valor");
    }

    #[test]
    fn ndb_es_ajuste_valor() {
        let mut v = crate::models::Venta::nuevo("test.csv", "2026-11");
        v.tpo_doc = "NDB".to_string();
        v.cantidad = 0.0;
        derivar_campos(&mut v);
        assert_eq!(v.tipo_operacion, "ajuste_valor");
    }

    #[test]
    fn f01_es_venta() {
        let mut v = crate::models::Venta::nuevo("test.csv", "2026-11");
        v.tpo_doc = "F01".to_string();
        v.cantidad = 10.0;
        derivar_campos(&mut v);
        assert_eq!(v.tipo_operacion, "venta");
    }

    #[test]
    fn bdi_es_venta() {
        let mut v = crate::models::Venta::nuevo("test.csv", "2026-11");
        v.tpo_doc = "BDI".to_string();
        v.cantidad = 5.0;
        derivar_campos(&mut v);
        assert_eq!(v.tipo_operacion, "venta");
    }
}
