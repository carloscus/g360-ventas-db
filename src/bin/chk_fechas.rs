// Reconstruye `ventas` para agregar los CHECK de formato de fecha.
//
// Por qué una reconstrucción y no un ALTER: SQLite no puede agregar una
// constraint a una tabla existente. El procedimiento renombra la original como
// `ventas_pre_chk` (respaldo real), crea la nueva desde `schema::CREATE_TABLE_SQL`
// -- que ya trae los CHECK -- copia, suelta los indices de la renombrada para
// recrearlos sobre la nueva y, al final, repara las vistas.
//
// Por qué importa: hay ~45 comparaciones de fecha como texto en este proyecto
// (`WHERE fecha_orig BETWEEN ? AND ?`, `ORDER BY fecha_orig`, `MAX/MIN`.)
// Funcionan solo porque la columna esta entera en ISO. Con `fec_cargo` en
// dd/mm/yyyy un rango sobre esa columna devolvia CERO filas sin dar error. Con
// el CHECK el estado roto es imposible de escribir.
//
// OJO CON LAS VISTAS: desde SQLite 3.25, `ALTER TABLE t RENAME TO t2` REESCRIBE
// las referencias a `t` que encuentra en vistas, triggers y FKs. Sin el
// `PRAGMA legacy_alter_table=ON` de abajo, las 14 vistas quedan apuntando a
// `ventas_pre_chk`; 9 de ellas se crean con `CREATE VIEW IF NOT EXISTS`, asi que
// `init_pool` NO las recrea y quedan rotas para siempre en cuanto el respaldo se
// descarta. Con el pragma, el rename es opaco y las vistas siguen apuntando a
// `ventas`, que al final es la tabla nueva. `_verificar_vistas` lo comprueba.
//
// `CREATE TABLE IF NOT EXISTS` (en `init_pool`) solo aplica a bases nuevas: no
// altera una tabla existente. Esta herramienta es el camino para las instaladas.
//
// USO (con la app cerrada, por el lock de escritura):
//     cargo run --release --bin chk_fechas            # informa
//     cargo run --release --bin chk_fechas --aplicar  # reconstruye
//     cargo run --release --bin chk_fechas --revertir # vuelve atras
//     cargo run --release --bin chk_fechas --descartar-vieja  # borra el respaldo
//     cargo run --release --bin chk_fechas --db RUTA  # otra base (pruebas)
use anyhow::{Context, Result, bail};
use sqlx::{Acquire, sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const VIEJA: &str = "ventas_pre_chk";

/// donde se guarda el SQL de los indices originales.
///
/// Al aplicar hay que soltar los indices de la tabla renombrada (sus nombres
/// chocan con los del CREATE INDEX nuevo) y eso deja al RESPALDO sin indices. Si
/// el revert solo hace DROP + RENAME, vuelve una `ventas` desnuda: el mensaje
/// "funciona" pero cada consulta paga un full-scan de 2.8M de filas. Guardando
/// el SQL aca, la reversion restaura exactamente lo que habia.
const TABLA_INDICES: &str = "_chk_indices_sql";

/// Formato que exige el CHECK: forma GLOB + calendario de `date()`.
/// `COALESCE(date(c),'')` no es cosmetico: `date('2024-13-01')` es NULL y en un
/// CHECK un NULL cuenta como SATISFECHO, asi que sin el COALESCE pasaria basura.
const GLOB_ISO: &str = "[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]";

/// Columnas de fecha y si admiten NULL/vacio. Solo `fecha_orig` es NOT NULL en
/// el esquema; `fec_cargo` viene 97% vacia porque el ERP la completa en parte de
/// las NC/ND y nunca en facturas (dato faltante de origen, no bug).
const COLS_FECHA: [(&str, bool); 4] = [
    ("fecha_orig", false),
    ("fecha_ref", true),
    ("fecha_venc", true),
    ("fec_cargo", true),
];

async fn checks_de(conn: &SqlitePool, tabla: &str) -> i64 {
    let sql: Option<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='table' AND name=?")
            .bind(tabla)
            .fetch_optional(conn)
            .await
            .ok()
            .flatten();
    sql.map(|s| s.matches("CHECK").count() as i64).unwrap_or(0)
}

async fn filas(conn: &SqlitePool, tabla: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {tabla}"))
        .fetch_one(conn)
        .await
        .unwrap_or(0)
}

/// Filas que impedirian la copia (deberia ser 0).
async fn violadores(conn: &SqlitePool) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for (col, admite_vacia) in COLS_FECHA {
        let where_ = if admite_vacia {
            format!("{col} IS NOT NULL AND {col} <> ''")
        } else {
            col.to_string()
        };
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM ventas WHERE {where_} AND NOT ({col} GLOB '{GLOB_ISO}' AND COALESCE(date({col}), '') = {col})"
        ))
        .fetch_one(conn)
        .await
        .unwrap_or(0);
        out.push((col.to_string(), n));
    }
    out
}

/// Recrea las vistas dejando TODAS apuntando a `ventas`.
///
/// Red de seguridad del `legacy_alter_table`: si el rename hubiera reescrito sus
/// definiciones, esto las deshace. Se ejecutan en orden inverso al de creacion
/// original (por `rowid`) y con los DROP primero, porque `vw_comparativo_linea`
/// depende de `vw_comparativo_ventas` y SQLite no permite dropear una vista de la
/// que otra depende mientras exista.
async fn reparar_vistas(con: &mut sqlx::SqliteConnection) -> Result<usize> {
    let sueltas: Vec<(String, String)> =
        sqlx::query_as("SELECT name, sql FROM sqlite_master WHERE type='view' ORDER BY rowid")
            .fetch_all(&mut *con)
            .await?;
    let reb_anadas: usize = sueltas
        .iter()
        .filter(|(_, sql)| sql.contains(VIEJA))
        .count();
    if sueltas.is_empty() {
        return Ok(0);
    }
    for (nombre, _) in &sueltas {
        sqlx::query(&format!("DROP VIEW IF EXISTS \"{nombre}\""))
            .execute(&mut *con)
            .await?;
    }
    for (_, sql) in sueltas.iter().rev() {
        sqlx::query(sql).execute(&mut *con).await.with_context(|| {
            format!("recreando vista: {}", &sql.chars().take(60).collect::<String>())
        })?;
    }
    Ok(reb_anadas)
}

/// Cada vista tiene que responder. Una vista que quedo apuntando a la tabla
/// equivocada no da error al recrearse: solo cuando se consulta. Por eso se
/// ejecuta un SELECT real contra cada una.
async fn verificar_vistas(pool: &SqlitePool) -> Result<usize> {
    let nombres: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='view' ORDER BY name")
            .fetch_all(pool)
            .await?;
    let mut malas = Vec::new();
    for n in &nombres {
        let q = format!("SELECT 1 FROM \"{n}\" LIMIT 1");
        if let Err(e) = sqlx::query(&q).fetch_optional(pool).await {
            malas.push(format!("{n}: {e}"));
        }
    }
    if !malas.is_empty() {
        error!("vistas rotas ({}):", malas.len());
        for m in &malas {
            error!("  {m}");
        }
        bail!("vistas rotas tras la reconstruccion");
    }
    Ok(nombres.len())
}

fn nombre_de_index(sql: &str) -> Option<String> {
    // "CREATE [UNIQUE] INDEX nombre ON tabla(...)"
    let upper = sql.to_uppercase();
    let pos = upper.find(" INDEX ")?;
    let resto = &sql[pos + 7..];
    let fin = resto.find(" ON ").or_else(|| resto.find('('))?;
    Some(resto[..fin].trim().trim_matches('"').replace('"', ""))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new("info"))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let aplicar = args.iter().any(|a| a == "--aplicar");
    let revertir = args.iter().any(|a| a == "--revertir");
    let descartar = args.iter().any(|a| a == "--descartar-vieja");

    // `--db` tiene que ganarle a todo: es la unica forma de ensayar sin tocar
    // la base de la app.
    let db_override = args
        .iter()
        .position(|a| a == "--db")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let pool: SqlitePool = match &db_override {
        Some(ruta) => {
            let opts = SqliteConnectOptions::from_str(&format!("sqlite://{ruta}?mode=rwc"))
                .map_err(|e| anyhow::anyhow!("--db invalido ({ruta}): {e}"))?
                .create_if_missing(true)
                .busy_timeout(Duration::from_secs(30));
            SqlitePool::connect_with(opts).await?
        }
        None => g360_db_ventas::db::writer::init_pool().await?,
    };
    let ruta: String =
        sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|_| "(desconocida)".into());
    info!("DB: {ruta}");
    if db_override.is_some() {
        warn!("--db: base indicada a mano, NO la de la app");
    }

    let tiene_vieja: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name=?")
            .bind(VIEJA)
            .fetch_optional(&pool)
            .await?;
    let checks = checks_de(&pool, "ventas").await;
    let total = filas(&pool, "ventas").await;

    if descartar {
        if tiene_vieja.is_none() {
            info!("no hay {VIEJA}: nada que descartar.");
            return Ok(());
        }
        // Las vistas se comprueban antes de borrar el respaldo: si alguna
        // apuntara aca, recien ahora se veria y todavia se puede arreglar.
        verificar_vistas(&pool).await?;
        let reb: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND sql LIKE '%{VIEJA}%'"
        ))
        .fetch_one(&pool)
        .await?;
        if reb > 0 {
            error!("{reb} vista(s) apuntan a {VIEJA}: NO se descarta. Repara las vistas primero.");
            return Ok(());
        }
        sqlx::query(&format!("DROP TABLE {VIEJA}")).execute(&pool).await?;
        sqlx::query(&format!("DROP TABLE IF EXISTS {TABLA_INDICES}"))
            .execute(&pool)
            .await?;
        info!("respaldo {VIEJA} descartado.");
        verificar_vistas(&pool).await?;
        return Ok(());
    }

    if revertir {
        if tiene_vieja.is_none() {
            info!("no hay {VIEJA}: nada que revertir.");
            return Ok(());
        }
        // Orden critico, y TODO en una sola conexion y una sola transaccion:
        //  1. `ventas` (la nueva, con CHECK) se borra -> sus indices desaparecen
        //     y liberan los nombres.
        //  2. El respaldo se renombra a `ventas`.
        //  3. Se restauran los indices desde la tabla de auditoria.
        //
        // Por que UNA conexion: con `execute(&pool)` cada sentencia puede salir
        // por una conexion distinta, y el `DROP` y el `RENAME` pueden no verse
        // entre si -> "there is already another table named ventas". Con
        // `legacy_alter_table` ademas hay que setear el PRAGMA en la MISMA
        // conexion que hace el RENAME, o no protege a las vistas.
        let mut con = pool.acquire().await?;
        let guardados: Vec<String> =
            sqlx::query_scalar(&format!("SELECT sql FROM {TABLA_INDICES} ORDER BY rowid"))
                .fetch_all(&mut *con)
                .await
                .unwrap_or_default();
        sqlx::query("PRAGMA legacy_alter_table=ON")
            .execute(&mut *con)
            .await?;
        let mut tx = con.begin().await?;
        // Si una corrida anterior murio entre el DROP y el RENAME, `ventas` ya
        // no esta: se sigue adelante en vez de abortar con "no such table".
        let existe_ventas: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ventas'")
                .fetch_one(&mut *tx)
                .await?;
        if existe_ventas > 0 {
            sqlx::query("DROP TABLE ventas").execute(&mut *tx).await?;
        }
        sqlx::query(&format!("ALTER TABLE {VIEJA} RENAME TO ventas"))
            .execute(&mut *tx)
            .await?;
        for sql in &guardados {
            sqlx::query(sql)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("restaurando indice: {sql}"))?;
        }
        sqlx::query(&format!("DROP TABLE IF EXISTS {TABLA_INDICES}"))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        sqlx::query("PRAGMA legacy_alter_table=OFF")
            .execute(&mut *con)
            .await?;

        let n = verificar_vistas(&pool).await?;
        let ind: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='ventas' AND sql IS NOT NULL",
        )
        .fetch_one(&pool)
        .await?;
        info!(
            "revertido: {VIEJA} volvio a ser `ventas`.  filas {total},  indices {ind} (guardados {}),  vistas {n},  CHECKs: {}",
            guardados.len(),
            checks_de(&pool, "ventas").await
        );
        if ind == 0 || ind as usize != guardados.len() {
            bail!("revertido con {ind} indices, se esperaban {}: el respaldo quedo sin indices", guardados.len());
        }
        return Ok(());
    }

    let n_ind = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='ventas' AND sql IS NOT NULL",
    )
    .fetch_one(&pool)
    .await?;
    info!("  filas: {total}   indices: {n_ind}   CHECKs: {checks}");
    if tiene_vieja.is_some() {
        warn!("hay un {VIEJA} de una corrida previa: usá --descartar-vieja antes de reintentar");
        return Ok(());
    }
    if checks > 0 {
        info!("  la tabla ya tiene CHECK: nada que hacer.");
        return Ok(());
    }

    let viol = violadores(&pool).await;
    for (col, n) in &viol {
        info!("  {col:<11} violan el CHECK: {n:>10}");
    }
    let total_viol: i64 = viol.iter().map(|(_, n)| n).sum();
    if total_viol > 0 {
        error!("HAY VIOLACIONES: normaliza primero con normaliza_fechas.");
        return Ok(());
    }
    info!("  0 violaciones: se puede reconstruir.");
    if !aplicar {
        info!("\nsolo informe. Para escribir: --aplicar");
        return Ok(());
    }

    let indices: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name='ventas' AND sql IS NOT NULL",
    )
    .fetch_all(&pool)
    .await?;
    info!("\nindices a recrear: {}", indices.len());

    let t0 = Instant::now();
    // En una transaccion: `PRAGMA foreign_keys` es no-op y no hay FKs que
    // referencien `ventas` (verificado: 0 triggers, 0 claves foraneas).
    let mut tx = pool.begin().await?;
    sqlx::query("PRAGMA legacy_alter_table=ON")
        .execute(&mut *tx)
        .await?;

    // La ORIGINAL se renombra, no se borra: ese es el respaldo que permite
    // revertir sin volver a copiar 2.8M de filas. Con legacy_alter_table el
    // rename no toca vistas ni el SQL de los indices, asi que siguen diciendo
    // "ON ventas" y el unico conflicto son los NOMBRES.
    sqlx::query(&format!("ALTER TABLE ventas RENAME TO {VIEJA}"))
        .execute(&mut *tx)
        .await?;

    // Copia de los SQL de indices para que `--revertir` no devuelva una tabla
    // desnuda (el SQL guardado dice "ON ventas", que es justo donde vuelve).
    sqlx::query(&format!("DROP TABLE IF EXISTS {TABLA_INDICES}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query(&format!("CREATE TABLE {TABLA_INDICES} (id INTEGER PRIMARY KEY, sql TEXT)"))
        .execute(&mut *tx)
        .await?;
    for sql in &indices {
        sqlx::query(&format!("INSERT INTO {TABLA_INDICES} (sql) VALUES (?)"))
            .bind(sql)
            .execute(&mut *tx)
            .await?;
    }

    for sql in &indices {
        let nombre = nombre_de_index(sql)
            .with_context(|| format!("no pude leer el nombre del indice: {sql}"))?;
        sqlx::query(&format!("DROP INDEX IF EXISTS \"{nombre}\""))
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(g360_db_ventas::db::schema::CREATE_TABLE_SQL)
        .execute(&mut *tx)
        .await?;
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('ventas')")
        .fetch_all(&mut *tx)
        .await?;
    let lista = cols.join(", ");
    info!("  copiando filas...");
    sqlx::query(&format!(
        "INSERT INTO ventas ({lista}) SELECT {lista} FROM {VIEJA}"
    ))
    .execute(&mut *tx)
    .await?;
    // El conteo va por la TRANSACCION: por el pool vernia otra conexion, que no
    // ve `ventas` hasta el commit.
    let copiadas: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&mut *tx)
        .await?;
    if copiadas != total {
        bail!("copiadas {copiadas} != {total}: se aborta y no se commitea");
    }

    for sql in &indices {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    let reparadas = reparar_vistas(&mut *tx).await?;
    sqlx::query("PRAGMA legacy_alter_table=OFF")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    info!("  copiadas: {copiadas} de {total}");
    info!("  indices recreados: {}", indices.len());
    info!("  vistas reparadas: {reparadas}");
    info!(
        "  CHECKs ahora: {}   indice: {:.1}s",
        checks_de(&pool, "ventas").await,
        t0.elapsed().as_secs_f64()
    );
    verificar_vistas(&pool).await?;
    info!("\nrespaldo conservado como {VIEJA}. Verificá y después:");
    info!("  cargo run --release --bin chk_fechas --descartar-vieja");
    Ok(())
}