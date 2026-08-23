use anyhow::Result;
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Convert XLS to CSV using Python xlrd
fn xls_to_csv(xls_path: &PathBuf) -> Result<PathBuf> {
    let csv_path = xls_path.with_extension("csv");
    let py_code = format!(
        r#"import xlrd, csv, sys
try:
    wb = xlrd.open_workbook(r'{}')
    s = wb.sheet_by_index(0)
    with open(r'{}', 'w', newline='', encoding='utf-8') as f:
        w = csv.writer(f)
        for r in range(s.nrows):
            w.writerow([str(s.cell_value(r, c)).strip() for c in range(s.ncols)])
    print(f'Converted {{s.nrows}} rows')
except Exception as e:
    print(f'Error: {{e}}', file=sys.stderr)
    sys.exit(1)
"#,
        xls_path.display(),
        csv_path.display()
    );
    let output = std::process::Command::new("python")
        .arg("-c")
        .arg(&py_code)
        .output()
        .map_err(|e| anyhow::anyhow!("python: {:?}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("XLS conversion failed: {}", stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    info!("  XLS->CSV: {}", stdout.trim());
    Ok(csv_path)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!("g360-db-ventas - Normalize -> SQLite");

    let raw_dir = g360_db_ventas::config::raw_dir();

    // Find all data files (CSV and XLS)
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&raw_dir) {
        for e in entries.flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext == "csv" {
                files.push(path);
            }
        }
    }
    files.sort();

    if files.is_empty() {
        error!("No data files found in {}", raw_dir.display());
        return Ok(());
    }

    info!("Found {} files", files.len());

    let pool = g360_db_ventas::db::writer::init_pool().await?;
    let mut total = 0usize;

    for file in &files {
        let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");

        // Convert XLS to CSV if needed
        let csv_path = if ext == "xls" || ext == "xlsx" {
            info!(
                "  Converting XLS: {}",
                file.file_name().unwrap().to_string_lossy()
            );
            match xls_to_csv(file) {
                Ok(p) => Some(p),
                Err(e) => {
                    error!("  XLS conversion failed: {}", e);
                    continue;
                }
            }
        } else {
            None
        };

        let path_to_parse = csv_path.as_ref().unwrap_or(file);
        let fname = path_to_parse
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        info!("  Parse: {}", fname);

        // Try full format first, then simple
        match g360_db_ventas::processor::parser::parse_export_csv(path_to_parse) {
            Ok(mut ventas) => {
                if ventas.is_empty() {
                    info!("  Full format returned 0 rows, trying simple format");
                    match g360_db_ventas::processor::parser::parse_simple_csv(path_to_parse) {
                        Ok(v2) => {
                            ventas = v2;
                        }
                        Err(e) => {
                            error!("  Simple format also failed: {}", e);
                            continue;
                        }
                    }
                }
                let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas).await?;
                total += n;
                info!("  +{} rows", n);
            }
            Err(e) => {
                error!("  Parse error: {}", e);
                continue;
            }
        }
    }

    info!("Total in DB: {}", total);
    Ok(())
}
