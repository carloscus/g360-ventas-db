// Probe transporte HTTP puro: dia normal + dia pesado + split
use g360_db_ventas::browser::captor::MonthRange;
use g360_db_ventas::browser::http::CipsaHttp;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();

    let raw = PathBuf::from("probe_out");
    std::fs::create_dir_all(&raw)?;

    let t0 = std::time::Instant::now();
    let s = CipsaHttp::new()?;
    println!("[{:>5}s] login...", t0.elapsed().as_secs());
    s.login().await?;
    println!("[{:>5}s] login OK", t0.elapsed().as_secs());

    for (label, d) in [("normal", (2026, 8, 15)), ("pesado", (2024, 1, 19))] {
        let day = MonthRange {
            start: chrono::NaiveDate::from_ymd_opt(d.0, d.1, d.2).unwrap(),
            end: chrono::NaiveDate::from_ymd_opt(d.0, d.1, d.2).unwrap(),
            label: format!("{}-{:02}-{:02}", d.0, d.1, d.2),
        };
        println!("[{:>5}s] descargando {} {}...", t0.elapsed().as_secs(), label, day.label);
        match tokio::time::timeout(std::time::Duration::from_secs(300), s.download(&day, &raw)).await {
            Ok(Ok(p)) => {
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                println!("[{:>5}s] {} OK {} bytes", t0.elapsed().as_secs(), label, size);
            }
            Ok(Err(e)) => println!("[{:>5}s] {} FAIL: {e:#}", t0.elapsed().as_secs(), label),
            Err(_) => println!("[{:>5}s] {} TIMEOUT", t0.elapsed().as_secs(), label),
        }
    }
    Ok(())
}
