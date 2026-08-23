use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    println!("g360-db-ventas v0.1.0");
    println!("Automatic sales capture CIPSA -> SQLite + Supabase");
    let d = g360_db_ventas::config::data_dir();
    println!("Data dir: {}", d.display());
    std::fs::create_dir_all(g360_db_ventas::config::raw_dir())?;
    std::fs::create_dir_all(g360_db_ventas::config::logs_dir())?;
    let db = g360_db_ventas::config::db_path();
    if db.exists() {
        let p = g360_db_ventas::db::writer::init_pool().await?;
        let t = g360_db_ventas::db::writer::count_ventas(&p).await?;
        println!("Database: {} records", t);
    }
    println!();
    println!("Commands:");
    println!("  cargo run --bin capture       # 1 month");
    println!("  cargo run --bin batch         # 24 months");
    println!("  cargo run --bin normalize     # CSVs -> SQLite");
    println!("  cargo run --bin upload        # SQLite -> Supabase");
    println!("  cargo run --bin query         # Query DB");
    Ok(())
}
