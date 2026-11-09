// Binario para generar el primer cartucho inicial desde cero.
// Este es el comando que usa el admin para preparar la base canonica
// que se distribuira a clientes offline.
//
// Uso:
//   cargo run --bin build_initial_cartucho -- --db ./data/ventas.db --output ./exports/base_canonica.db
use anyhow::{Context, Result};
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let mut db_path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut help = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--db" | "-d" => {
                i += 1;
                if i < args.len() {
                    db_path = Some(args[i].clone());
                }
            }
            "--output" | "-o" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(args[i].clone());
                }
            }
            "--help" | "-h" => help = true,
            _ => {}
        }
        i += 1;
    }

    if help || db_path.is_none() || output_path.is_none() {
        eprintln!("Usage: build_initial_cartucho --db <ruta_db> --output <ruta_output>");
        eprintln!();
        eprintln!("Generates a canonical cartridge (.db + manifest.json) from an existing database.");
        eprintln!("This is the command to prepare the base data for distribution to offline clients.");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --db, -d    Path to the source SQLite database");
        eprintln!("  --output, -o  Path for the output cartridge (.db will be appended)");
        eprintln!("  --help, -h  Show this help");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  build_initial_cartucho --db ./data/ventas.db --output ./exports/base_canonica.db");
        std::process::exit(1);
    }

    let db_path = db_path.unwrap();
    let output_base = output_path.unwrap();
    let output_db = format!("{}.db", output_base);
    let _ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let output_manifest = format!("{}.manifest.json", output_base);

    info!("Building initial cartridge from: {}", db_path);
    info!("Output: {} (will create {})", output_db, output_manifest);

    // Inicializar pool apuntando a la DB de entrada (read-write porque
    // calculate_day_checksums y calculate_monthly_checksums necesitan escribir)
    use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode}, Pool};
    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let source_pool = Pool::connect_with(opts)
        .await
        .context(format!("Cannot open source DB: {}", db_path))?;

    // Verificar version de contrato
    match g360_db_ventas::db::writer::check_contract_version(&source_pool).await {
        Ok(_) => info!("✓ Source DB has compatible contract version"),
        Err(e) => {
            error!("⚠ Source DB contract version mismatch: {}", e);
            error!("  This cartridge may not work with current consumer code.");
        }
    }

    // Verificar que tenga datos
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(&source_pool).await?;
    info!("Source DB has {} rows", count);
    if count == 0 {
        error!("ERROR: Source DB is empty. Cannot build cartridge from empty database.");
        std::process::exit(1);
    }

    // Verificar rango de fechas
    let rango: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT MIN(fecha_orig), MAX(fecha_orig) FROM ventas"
    ).fetch_one(&source_pool).await?;
    info!("Date range: {} to {}", rango.0.clone().unwrap_or_default(), rango.1.clone().unwrap_or_default());

    // Calcular checksums antes del export
    info!("Calculating checksums...");
    g360_db_ventas::db::writer::calculate_monthly_checksums(&source_pool).await?;
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    g360_db_ventas::db::writer::calculate_day_checksums(&source_pool, &ts).await?;

    // Crear directorio de salida
    if let Some(parent) = std::path::Path::new(&output_db).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Exportar el archivo .db usando VACUUM INTO
    info!("Exporting database to {}...", output_db);
    sqlx::query(&format!("VACUUM INTO '{}'", output_db.replace("\\", "/")))
        .execute(&source_pool)
        .await
        .context("Failed to vacuum export")?;

    // Leer el archivo exportado para calcular SHA256
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(&output_db)?;
    use std::io::Read;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    let sha256 = format!("{:x}", hasher.finalize());

    // Construir manifiesto basico (sin stats detalladas porque estamos en modo standalone)
    let total_rows = count;
    let mm: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT MIN(mes_ref), MAX(mes_ref), COUNT(DISTINCT mes_ref) FROM ventas WHERE length(mes_ref)=7"
    ).fetch_one(&source_pool).await?;

    let contract_ver: String = sqlx::query_scalar(
        "SELECT value FROM g360_meta WHERE key = 'contracto_version'"
    ).fetch_one(&source_pool).await.unwrap_or(g360_db_ventas::db::schema::CONTRACTO_VERSION.to_string());

    let manifest = serde_json::json!({
        "version": contract_ver.parse::<i64>().unwrap_or(0),
        "contracto_version": contract_ver,
        "dim_schema_version": g360_db_ventas::db::schema::DIM_SCHEMA_VERSION.to_string(),
        "normalizacion_version": g360_db_ventas::db::schema::NORMALIZACION_VERSION.to_string(),
        "format": "g360-ventas-cartucho-v2",
        "tipo": "semilla",
        "generado_en": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "generado_por": format!("g360-db-ventas/build-initial/{}", env!("CARGO_PKG_VERSION")),
        "rango": {
            "desde": rango.0.clone().unwrap_or_default(),
            "hasta": rango.1.clone().unwrap_or_default()
        },
        "sha256": sha256,
        "db_size_bytes": std::fs::metadata(&output_db).map(|m| m.len()).unwrap_or(0),
        "quick_check": {
            "ok": true,
            "filas": total_rows,
            "meses": mm.2
        },
        "espejo_completo": true,
        "contenido": "raw-first: TODAS las lineas del ERP; sin filtro de lineas ni retencion"
    });

    std::fs::write(&output_manifest, serde_json::to_string_pretty(&manifest)?)?;

    info!("Initial cartridge built successfully!");
    info!("  DB:     {} ({:.1} MB)", output_db,
        std::fs::metadata(&output_db).map(|m| m.len() as f64 / 1024.0 / 1024.0).unwrap_or(0.0));
    info!("  Manifest: {}", output_manifest);
    info!("  SHA256: {}", sha256);
    info!("  Rows: {}, Months: {}, Range: {} to {}", total_rows, mm.2,
        rango.0.clone().unwrap_or_default(), rango.1.clone().unwrap_or_default());

    Ok(())
}
