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
    pub estado_linea: String,
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
    pub soles: f64,
    pub dolares: f64,
    pub precio_unitario: f64,
    // Tiempo
    pub anho: i32,
    pub mes: i32,
    pub fecha_orig: NaiveDate,
    pub fecha_ref: Option<String>,
    pub fecha_venc: Option<String>,
    pub fec_cargo: Option<String>,
    // Ubicacion y canal
    pub cod_sucursal: String,
    pub nom_sucursal: String,
    pub departamento: String,
    pub provincia: String,
    pub distrito: String,
    pub canal_dist: String,
    // Vendedor
    pub id_vendedor: String,
    pub nom_vendedor: String,
    // Pedido
    pub id_pedido: String,
    // Metadata
    pub file_source: String,
    pub mes_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CaptureStatus {
    Idle,
    LoggingIn,
    Capturing {
        month: u32,
        total: u32,
        date_range: (String, String),
    },
    Parsing,
    Uploading,
    Done {
        total_records: usize,
    },
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureResult {
    pub months_completed: Vec<String>,
    pub total_files: usize,
    pub total_records: usize,
    pub errors: Vec<(String, String)>,
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
            estado_linea: String::new(),
            id_cliente: String::new(),
            doc_cliente: String::new(),
            nom_cliente: String::new(),
            tpo_doc: String::new(),
            serie_doc: String::new(),
            nro_doc: String::new(),
            referencia: String::new(),
            moneda: String::from("Soles"),
            cantidad: 0.0,
            soles: 0.0,
            dolares: 0.0,
            precio_unitario: 0.0,
            anho: 0,
            mes: 0,
            fecha_orig: NaiveDate::default(),
            fecha_ref: None,
            fecha_venc: None,
            fec_cargo: None,
            cod_sucursal: String::new(),
            nom_sucursal: String::new(),
            departamento: String::new(),
            provincia: String::new(),
            distrito: String::new(),
            canal_dist: String::new(),
            id_vendedor: String::new(),
            nom_vendedor: String::new(),
            id_pedido: String::new(),
            file_source: file_source.to_string(),
            mes_ref: mes_ref.to_string(),
        }
    }
}
