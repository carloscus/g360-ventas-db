// Conversion XLS -> CSV puro Rust con calamine (sin Python/xlrd).
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tracing::info;

use calamine::{open_workbook_auto, Data, Reader};

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
    for row in range.rows() {
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
    info!("XLS->CSV (calamine): {} filas -> {}", rows, csv.display());
    if rows == 0 {
        return Err(anyhow!("conversion no produjo filas: {}", xls.display()));
    }
    Ok(rows)
}
