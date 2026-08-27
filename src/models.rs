use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Estructura de una venta individual (44 columnas ERP)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Venta {
    // Identificadores
    pub id_articulo: String,
    pub original_sku: String,
    pub nom_articulo: String,
    pub id_linea: String,
    pub nom_linea: String,
    pub id_grupo: String,
    pub nom_grupo: String,
    pub id_tipo: String,
    pub nom_tipo: String,
    pub id_familia: String,
    pub nom_familia: String,
    // Cliente - id_cliente (codigo interno) -> nom_cliente -> doc_cliente (RUC)
    pub id_cliente: String,
    pub doc_cliente: String,
    pub nom_cliente: String,
    // Documento
    pub tpo_doc: String,
    pub serie_doc: String,
    pub nro_doc: String,
    pub referencia: String,
    // Montos
    pub moneda: String,
    pub cantidad: f64,
    /// Cantidad FAE del ERP: base sobre la que se aplico el descuento (puede diferir de cantidad
    /// en ajustes de valor; ej. vendi 1000 pero descuente solo 500 entregados).
    pub cantidad_fae: f64,
    pub soles: f64,
    pub dolares: f64,
    pub precio_unitario: f64,
    // Tiempo
    pub anho: i32,
    pub mes: i32,
    pub fecha_orig: NaiveDate,
    pub fecha_ref: Option<String>,
    pub fecha_venc: Option<String>,
    // Ubicacion y canal
    pub cod_sucursal: String,
    pub nom_sucursal: String,
    pub departamento: String,
    pub provincia: String,
    pub distrito: String,
    // Vendedor
    pub id_vendedor: String,
    pub nom_vendedor: String,
    // Pedido
    pub id_pedido: String,
    // Metadata
    pub file_source: String,
    pub mes_ref: String,
    // Derivados para CRM
    pub tipo_operacion: String,      // venta | devolucion | ajuste_valor | nota_debito
    pub factura_ref_serie: String,   // serie de la factura referenciada (si NCR/NDB)
    pub factura_ref_nro: String,     // nro de la factura referenciada
    pub folio_unico: String,         // tpo_doc/serie-nro unico para dedup y lookups
}

impl Venta {
    pub fn nuevo(file_source: &str, mes_ref: &str) -> Self {
        Self {
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
            id_cliente: String::new(),
            doc_cliente: String::new(),
            nom_cliente: String::new(),
            tpo_doc: String::new(),
            serie_doc: String::new(),
            nro_doc: String::new(),
            referencia: String::new(),
            moneda: String::from("Soles"),
            cantidad: 0.0,
            cantidad_fae: 0.0,
            soles: 0.0,
            dolares: 0.0,
            precio_unitario: 0.0,
            anho: 0,
            mes: 0,
            fecha_orig: NaiveDate::default(),
            fecha_ref: None,
            fecha_venc: None,
            cod_sucursal: String::new(),
            nom_sucursal: String::new(),
            departamento: String::new(),
            provincia: String::new(),
            distrito: String::new(),
            id_vendedor: String::new(),
            nom_vendedor: String::new(),
            id_pedido: String::new(),
            file_source: file_source.to_string(),
            mes_ref: mes_ref.to_string(),
            tipo_operacion: String::new(),
            factura_ref_serie: String::new(),
            factura_ref_nro: String::new(),
            folio_unico: String::new(),
        }
    }
}
