// Normaliza a ISO (yyyy-mm-dd) las columnas de fecha que el ERP entrega en
// dd/mm/yyyy. Idempotente y no destructivo: una fecha que no se puede
// interpretar se conserva tal cual (es preferible un dato raro a perderlo).
//
// Por que existe: `fec_cargo`, `fecha_ref` y `fecha_venc` se guardaban como
// texto crudo del CSV, sin pasar por `parse_date`. El resultado fue 74.380
// filas con `fec_cargo` en dd/mm/yyyy mientras `fecha_orig` ya estaba en ISO.
// El SQL del proyecto compara fechas como texto, asi que un rango sobre una de
// esas columnas devuelve cero filas sin dar error:
//
//     BETWEEN '2015-01-01' AND '2015-12-31'  ->  0 filas
//     BETWEEN '01/01/2015' AND '31/12/2015'  ->  74.380 filas
//
// OJO sobre `fec_cargo`: viene 97% vacio y es correcto. El ERP solo lo completa
// en parte de las notas de credito (NCR ~37%, NDB ~14%) y nunca en facturas.
// Esta migracion NO rellena vacios: solo convierte los valores que ya existen.
//
// USO (con la app cerrada, por el lock de escritura):
//     cargo run --release --bin normaliza_fechas            # informa
//     cargo run --release --bin normaliza_fechas --aplicar  # escribe
//     cargo run --release --bin normaliza_fechas --revertir # restaura
//     cargo run --release --bin normaliza_fechas --db RUTA  # otra base (pruebas)
//
// `--db` existe porque `config::db_path()` resuelve siempre a
// `%APPDATA%\g360-db-ventas\data\historial.db` (dirs::config_dir + nombre fijo)
// y NO hay variable de entorno para cambiarlo. Sin `--db` no hay forma de
// ensayar sobre una copia:吹 uno cree que esta probando y esta tocando
// produccion. El pool por defecto se resuelve igual que el de la app.
use anyhow::Result;
use sqlx::{Row, sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Acepta los mismos formatos que `processor::parser::parse_date`, mas el ISO
/// con timestamp pegado ("2024-01-15 10:30:00"), que no llega del ERP pero
/// aparece en exportaciones manuales.
const FORMATOS: [&str; 5] = [
    "%Y-%m-%d",
    "%d-%m-%Y",
    "%d/%m/%Y",
    "%Y/%m/%d",
    "%Y-%m-%d %H:%M:%S",
];

const COLUMNAS: [&str; 3] = ["fec_cargo", "fecha_ref", "fecha_venc"];
const LOG_TABLA: &str = "_migracion_fechas_iso";

fn a_iso(valor: &str) -> Option<String> {
    let s = valor.trim();
    if s.is_empty() {
        return None;
    }
    for f in FORMATOS {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, f) {
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }
    None
}

fn g_iso(s: &str) -> bool {
    s.len() == 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[7] == b'-'
        && s.as_bytes()
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();
    g360_db_ventas::config::load_dotenv();

    let aplicar = std::env::args().any(|a| a == "--aplicar");
    let revertir = std::env::args().any(|a| a == "--revertir");

    // `--db RUTA` tiene que ganarle a todo: es la unica forma de ensayar sin
    // tocar la base de la app.
    let db_override = std::env::args()
        .position(|a| a == "--db")
        .and_then(|i| std::env::args().nth(i + 1));

    let pool: SqlitePool = match &db_override {
        Some(ruta) => {
            let opts = SqliteConnectOptions::from_str(&format!("sqlite://{ruta}?mode=rwc"))
                .map_err(|e| anyhow::anyhow!("--db invalido ({ruta}): {e}"))?
                .create_if_missing(true)
                .busy_timeout(std::time::Duration::from_secs(30));
            SqlitePool::connect_with(opts).await?
        }
        None => g360_db_ventas::db::writer::init_pool().await?,
    };
    let db_path: String = sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|_| "(desconocida)".to_string());
    info!("DB: {db_path}");
    if db_override.is_some() {
        warn!("--db: se esta trabajando sobre una base indicada a mano, NO la de la app");
    }

    if revertir {
        return revertir_fechas(&pool).await;
    }

    for col in COLUMNAS {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
            .fetch_one(&pool)
            .await?;
        let con_valor: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM ventas WHERE {col} IS NOT NULL AND TRIM({col}) <> ''"
        ))
        .fetch_one(&pool)
        .await?;
        let ya_iso: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM ventas WHERE {col} IS NOT NULL AND TRIM({col}) <> '' \
             AND length(TRIM({col})) = 10 AND substr(TRIM({col}),5,1) = '-' \
             AND substr(TRIM({col}),8,1) = '-'"
        ))
        .fetch_one(&pool)
        .await?;
        let pendientes = con_valor - ya_iso;
        info!(
            "{col}: {con_valor} con valor de {total} ({:.2}%), {ya_iso} ya ISO, {pendientes} por migrar",
            100.0 * con_valor as f64 / total.max(1) as f64
        );
        if !aplicar || pendientes == 0 {
            continue;
        }

        // Log de reversion: una fila por (id, columna) con el valor anterior.
        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {LOG_TABLA} (\
               id INTEGER NOT NULL, col TEXT NOT NULL, valor TEXT, migrado_en TEXT \
               NOT NULL DEFAULT (datetime('now')), PRIMARY KEY (id, col))"
        ))
        .execute(&pool)
        .await?;

        let filas = sqlx::query(&format!(
            "SELECT id, {col} AS v FROM ventas \
             WHERE {col} IS NOT NULL AND TRIM({col}) <> '' \
               AND NOT (length(TRIM({col})) = 10 AND substr(TRIM({col}),5,1) = '-' \
                        AND substr(TRIM({col}),8,1) = '-')"
        ))
        .fetch_all(&pool)
        .await?;

        let mut hechas = 0i64;
        let mut conservadas = 0i64;
        let mut ilegibles = 0i64;
        let mut tx = pool.begin().await?;
        for f in filas.iter() {
            let id: i64 = f.try_get("id")?;
            let v: String = f.try_get("v")?;
            // La decision se toma sobre el valor ORIGINAL: `a_iso` devuelve
            // ISO por definicion, asi que preguntarle a su salida que si es
            // ISO da falso siempre y no migra nada.
            if g_iso(v.trim()) {
                conservadas += 1;
                continue;
            }
            match a_iso(&v) {
                Some(iso) => {
                    sqlx::query(&format!(
                        "INSERT OR IGNORE INTO {LOG_TABLA} (id, col, valor) VALUES (?, ?, ?)"
                    ))
                    .bind(id)
                    .bind(col)
                    .bind(&v)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(&format!("UPDATE ventas SET {col} = ? WHERE id = ?"))
                        .bind(&iso)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    hechas += 1;
                }
                None => {
                    // No se pudo interpretar: se deja como estaba y se avisa.
                    ilegibles += 1;
                    if ilegibles <= 5 {
                        warn!("{col}: valor ilegible conservado (id={id}): {v:?}");
                    }
                }
            }
        }
        tx.commit().await?;
        info!("  migradas {hechas}, ya ISO {conservadas}, ilegibles conservadas {ilegibles}");
    }

    if aplicar {
        info!("Reversión disponible: cargo run --release --bin normaliza_fechas --revertir");
    } else if !revertir {
        info!("Solo informe. Para escribir: --aplicar");
    }
    Ok(())
}

async fn revertir_fechas(pool: &sqlx::SqlitePool) -> Result<()> {
    let existe: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name=?")
            .bind(LOG_TABLA)
            .fetch_optional(pool)
            .await?;
    if existe.is_none() {
        info!("no hay log {LOG_TABLA}: nada que revertir.");
        return Ok(());
    }
    let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {LOG_TABLA}"))
        .fetch_one(pool)
        .await?;
    if n == 0 {
        info!("el log {LOG_TABLA} esta vacio: nada que revertir.");
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for col in COLUMNAS {
        let r = sqlx::query(&format!(
            "UPDATE ventas SET {col} = (SELECT valor FROM {LOG_TABLA} l \
             WHERE l.id = ventas.id AND l.col = '{col}') \
             WHERE id IN (SELECT id FROM {LOG_TABLA} WHERE col = '{col}')"
        ))
        .execute(&mut *tx)
        .await?;
        info!("  {col}: {} filas revertidas", r.rows_affected());
    }
    sqlx::query(&format!("DELETE FROM {LOG_TABLA}"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    error!("reversion completada");
    Ok(())
}
