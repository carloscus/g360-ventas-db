// Conversion XLS -> CSV puro Rust con calamine (sin Python/xlrd).
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tracing::{info, warn};

use calamine::{open_workbook_auto, Data, Reader};

/// Resultado de inspeccionar la columna SKU (ID_ARTICULO) de un XLS.
///
/// El ERP exporta los SKUs como TEXTO (preservando ceros a la izquierda:
/// `011019` ≠ `11019`). Si el archivo fue re-guardado en Excel con la columna
/// tipada como NUMERO, al leerlo con calamine las celdas llegan como
/// `Data::Float`/`Data::Int` y el cero inicial se perdio en origen — no hay
/// forma de recuperarlo. Esta inspeccion lo detecta ANTES de importar.
#[derive(Debug, Clone, Default)]
pub struct SkuColumnInspection {
    pub header_found: bool,
    pub col_index: Option<usize>,
    pub total_cells: usize,
    pub string_cells: usize,
    pub numeric_cells: usize,
    pub numeric_examples: Vec<String>,
}

impl SkuColumnInspection {
    /// True si alguna celda de la columna SKU llego como numero (riesgo de
    /// perdida de ceros a la izquierda).
    pub fn has_numeric_sku(&self) -> bool {
        self.numeric_cells > 0
    }

    /// Mensaje humano y accionable para mostrar al usuario.
    pub fn warning_message(&self) -> String {
        let ejemplos = if self.numeric_examples.is_empty() {
            String::new()
        } else {
            format!(" (ej: {})", self.numeric_examples.join(", "))
        };
        format!(
            "La columna SKU (ID_ARTICULO) está guardada como NÚMERO, no texto{}. \
             Excel pudo eliminar ceros a la izquierda (011019 → 11019) y crear SKUs falsos. \
             Descarga/vuelve a exportar el XLS del intranet SIN re-guardarlo en Excel.",
            ejemplos
        )
    }
}

/// Inspecciona la columna SKU de un XLS sin convertirlo ni insertar nada.
/// Busca la fila de cabecera y detecta si la columna `ID_ARTICULO` tiene
/// celdas numericas (riesgo de ceros perdidos).
pub fn inspect_sku_column(xls: &Path) -> Result<SkuColumnInspection> {
    let mut wb = open_workbook_auto(xls)
        .with_context(|| format!("abrir xls {}", xls.display()))?;
    let sheet_name = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("xls sin hojas: {}", xls.display()))?;
    let range = wb
        .worksheet_range(&sheet_name)
        .map_err(|e| anyhow!("leer hoja {sheet_name}: {e:?}"))?;

    let mut insp = SkuColumnInspection::default();

    // Ubicar la fila de cabecera y la columna ID_ARTICULO.
    // Coincide con "ID_ARTICULO" y descarta "NOM_ARTICULO".
    let mut header_row: Option<usize> = None;
    let mut sku_col: Option<usize> = None;
    for (ri, row) in range.rows().enumerate() {
        for (ci, cell) in row.iter().enumerate() {
            if let Data::String(s) = cell {
                let h = s.trim().to_uppercase().replace(' ', "_");
                if h == "ID_ARTICULO" || (h.contains("ARTICULO") && !h.contains("NOM")) {
                    sku_col = Some(ci);
                    header_row = Some(ri);
                    break;
                }
            }
        }
        if sku_col.is_some() {
            break;
        }
    }

    insp.header_found = sku_col.is_some();
    insp.col_index = sku_col;

    if let (Some(ci), Some(hr)) = (sku_col, header_row) {
        for row in range.rows().skip(hr + 1) {
            let Some(cell) = row.get(ci) else { continue };
            match cell {
                Data::Empty => {}
                Data::String(s) if s.trim().is_empty() => {}
                Data::String(_) => {
                    insp.string_cells += 1;
                    insp.total_cells += 1;
                }
                Data::Float(f) => {
                    insp.numeric_cells += 1;
                    insp.total_cells += 1;
                    if insp.numeric_examples.len() < 5 {
                        insp.numeric_examples.push(format!("{:.0}", f));
                    }
                }
                Data::Int(i) => {
                    insp.numeric_cells += 1;
                    insp.total_cells += 1;
                    if insp.numeric_examples.len() < 5 {
                        insp.numeric_examples.push(i.to_string());
                    }
                }
                _ => {
                    insp.total_cells += 1;
                }
            }
        }
    }

    Ok(insp)
}

/// Convierte `xls` a `csv` (primera hoja). Puro Rust, sin depender de Python.
pub fn xls_to_csv(xls: &Path, csv: &Path) -> Result<usize> {
    let mut wb = open_workbook_auto(xls)
        .with_context(|| format!("abrir xls {}", xls.display()))?;
    let sheet_name = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("xls sin hojas: {}", xls.display()))?;
    let range = wb
        .worksheet_range(&sheet_name)
        .map_err(|e| anyhow!("leer hoja {sheet_name}: {e:?}"))?;

    let mut w = csv::Writer::from_path(csv)
        .with_context(|| format!("crear csv {}", csv.display()))?;
    let mut rows = 0usize;
    // Deteccion de SKU numerico: el ERP exporta ID_ARTICULO como texto. Si llega
    // como numero, Excel pudo borrar ceros a la izquierda (011019 -> 11019).
    // Se localiza la columna en la primera fila (cabecera) y se cuentan celdas
    // numericas durante la misma pasada.
    let mut sku_col: Option<usize> = None;
    let mut numeric_sku_cells: usize = 0;
    let mut numeric_sku_examples: Vec<String> = Vec::new();

    for row in range.rows() {
        // Localizar la columna SKU en la cabecera (solo la primera vez).
        if sku_col.is_none() {
            for (ci, cell) in row.iter().enumerate() {
                if let Data::String(s) = cell {
                    let h = s.trim().to_uppercase().replace(' ', "_");
                    if h == "ID_ARTICULO" || (h.contains("ARTICULO") && !h.contains("NOM")) {
                        sku_col = Some(ci);
                        break;
                    }
                }
            }
        } else if let Some(ci) = sku_col {
            match row.get(ci) {
                Some(Data::Float(f)) => {
                    numeric_sku_cells += 1;
                    if numeric_sku_examples.len() < 5 {
                        numeric_sku_examples.push(format!("{:.0}", f));
                    }
                }
                Some(Data::Int(i)) => {
                    numeric_sku_cells += 1;
                    if numeric_sku_examples.len() < 5 {
                        numeric_sku_examples.push(i.to_string());
                    }
                }
                _ => {}
            }
        }

        let record: Vec<String> = row
            .iter()
            .map(|c| match c {
                Data::Empty => String::new(),
                Data::String(s) => s.trim().to_string(),
                Data::Float(f) => {
                    // Evitar 2021.0 -> "2021"
                    if f.fract() == 0.0 { format!("{:.0}", f) } else { f.to_string() }
                }
                Data::Int(i) => i.to_string(),
                Data::Bool(b) => b.to_string(),
                Data::DateTime(d) => d.to_string(),
                Data::DateTimeIso(s) => s.clone(),
                Data::DurationIso(s) => s.clone(),
                Data::Error(e) => format!("{e:?}"),
            })
            .collect();
        // Saltar filas totalmente vacías
        if record.iter().all(|s| s.is_empty()) {
            continue;
        }
        w.write_record(&record)
            .with_context(|| format!("escribir csv {}", csv.display()))?;
        rows += 1;
    }
    w.flush()?;

    if numeric_sku_cells > 0 {
        warn!(
            "SKU numerico detectado en {}: {} celdas de ID_ARTICULO llegaron como numero (ej: {}). \
             Posible perdida de ceros a la izquierda en origen (XLS re-guardado en Excel).",
            xls.display(),
            numeric_sku_cells,
            numeric_sku_examples.join(", ")
        );
    }

    info!("XLS->CSV (calamine): {} filas -> {}", rows, csv.display());
    if rows == 0 {
        return Err(anyhow!("conversion no produjo filas: {}", xls.display()));
    }
    Ok(rows)
}
