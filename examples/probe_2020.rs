use g360_db_ventas::browser::http::CipsaHttp;
use g360_db_ventas::config::load_dotenv;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_dotenv();
    let http = CipsaHttp::new()?;
    http.login().await?;
    // Probar si hay datos para enero 2020 (antes de MIN_AVAILABLE_DATE)
    let range = g360_db_ventas::browser::captor::MonthRange {
        start: chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
        end: chrono::NaiveDate::from_ymd_opt(2020, 1, 31).unwrap(),
        label: "2020-01".into(),
    };
    let raw = PathBuf::from("probe_out");
    std::fs::create_dir_all(&raw)?;
    match tokio::time::timeout(Duration::from_secs(600), http.download(&range, &raw)).await {
        Ok(Ok(xls)) => {
            let size = std::fs::metadata(&xls).map(|m| m.len()).unwrap_or(0);
            println!("OK: {} bytes para 2020-01 ({})", size, xls.display());
            // Convertir y contar filas
            let csv = raw.join("ventas_2020-01.csv");
            if size > 512 {
                let rows = g360_db_ventas::processor::xls::xls_to_csv(&xls, &csv)?;
                println!("filas: {rows}");
                let ventas = g360_db_ventas::processor::parser::parse_export_csv(&csv)?;
                println!("ventas filtradas: {}", ventas.len());
            }
        }
        Ok(Err(e)) => println!("FAIL: {e:#}"),
        Err(_) => println!("TIMEOUT"),
    }
    Ok(())
}
