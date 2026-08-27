// Test E2E pipeline completo: download 2021-01 -> calamine csv -> parse -> insert
use g360_db_ventas::browser::captor::MonthRange;
use g360_db_ventas::browser::http::CipsaHttp;
use chrono::{Datelike, NaiveDate};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();

    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;
    let (y, mo) = (2021u32, 1u32);
    let start = NaiveDate::from_ymd_opt(y as i32, mo, 1).unwrap();
    let end = if mo == 12 { NaiveDate::from_ymd_opt(y as i32 + 1, 1, 1).unwrap() - chrono::Duration::days(1) } else { NaiveDate::from_ymd_opt(y as i32, mo + 1, 1).unwrap() - chrono::Duration::days(1) };
    let label = format!("{y}-{mo:02}");
    let m = MonthRange { start, end, label: label.clone() };

    println!("=== E2E captura {label} (pipeline sin python) ===");
    let s = CipsaHttp::new()?;
    s.login().await?;
    let t0 = std::time::Instant::now();
    let xls = s.download(&m, &raw).await?;
    println!("descargado {:?} en {}s", xls.file_name().map(|f| f.to_string_lossy().to_string()), t0.elapsed().as_secs());

    // convertir con calamine (borrar csv previo para forzar reconversion)
    let csv_path = raw.join(format!("ventas_{label}.csv"));
    let _ = std::fs::remove_file(&csv_path);
    let rows = g360_db_ventas::processor::xls::xls_to_csv(&xls, &csv_path)?;
    println!("calamine rows={rows}");

    let ventas = g360_db_ventas::processor::parser::parse_export_csv(&csv_path)?;
    let soles: f64 = ventas.iter().map(|v| v.soles).sum();
    let dol: f64 = ventas.iter().map(|v| v.dolares).sum();
    println!("filtradas={} soles={soles:.2} dolares={dol:.2}", ventas.len());
    println!("(referencia cruda: 12419650.04/3422659.84 - filtrado debe ser menor)");

    let pool = g360_db_ventas::db::writer::init_pool().await?;
    let n = g360_db_ventas::db::writer::insert_ventas(&pool, &ventas).await?;
    let _ = g360_db_ventas::db::writer::dedup_ventas(&pool).await;
    use sqlx::Row;
    let r: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ventas WHERE mes_ref=?").bind(&label).fetch_one(&pool).await?;
    println!("insertadas={n} | DB {label} total={}", r.0);

    // Verificar dias
    let dias: Vec<String> = sqlx::query("SELECT DISTINCT fecha_orig FROM ventas WHERE mes_ref=? ORDER BY fecha_orig")
        .bind(&label).fetch_all(&pool).await?
        .iter().map(|row| row.get::<String,_>(0)).collect();
    println!("dias con data: {}/31", dias.len());
    Ok(())
}
