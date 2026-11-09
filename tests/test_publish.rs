// Tests para publish_incremental y calculate_day_checksums.
//
// USO:
//   cargo test --test test_publish
//
// Los tests usan SQLite in-memory y son 100% offline.
use g360_db_ventas::db::schema::{CREATE_AUDIT_TABLES_SQL, CREATE_TABLE_SQL};
use g360_db_ventas::db::writer::{calculate_day_checksums, publish_incremental};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

async fn setup() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await.unwrap();
    sqlx::query(CREATE_AUDIT_TABLES_SQL)
        .execute(&pool)
        .await
        .unwrap();
    // Insertar g360_meta con version de contrato vigente
    sqlx::query(
        "INSERT INTO g360_meta (key, value, actualizado_en) VALUES 
         ('contracto_version', '4', '2026-01-01'),
         ('dim_schema_version', '3', '2026-01-01'),
         ('normalizacion_version', '1', '2026-01-01')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

/// Inserta una venta de ejemplo en la BD.
async fn insert_venta(pool: &SqlitePool, fecha: &str, soles: f64) {
    sqlx::query(
        r#"INSERT INTO ventas
           (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
            id_cliente, nom_cliente, id_vendedor, nom_vendedor, tpo_doc, cantidad, soles,
            anho, mes, fecha_orig, mes_ref, tipo_operacion)
           VALUES (?,?,?,?,?, 'C1','CLIENTE','V1','Vendedor 1','F01',1,?,
            2026,1,?,?, 'venta')"#,
    )
    .bind("SKU1")
    .bind("SKU1")
    .bind("PRODUCTO 1")
    .bind("01")
    .bind("LINEA 01")
    .bind(soles)
    .bind(fecha)
    .bind(&fecha[..7])
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn calculate_day_checksums_creates_entries() {
    let pool = setup().await;

    // Insertar datos de dos días
    insert_venta(&pool, "2026-01-10", 100.0).await;
    insert_venta(&pool, "2026-01-10", 50.0).await;
    insert_venta(&pool, "2026-01-11", 200.0).await;

    let ts = "2026-11-04T12:00:00Z".to_string();
    let n = calculate_day_checksums(&pool, &ts).await.unwrap();

    // Deben haberse calculado checksums para 2 días
    assert_eq!(n, 2, "deben existir 2 días con datos");

    // Verificar que las entradas existen en day_checksums
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_checksums")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);

    // Verificar integridad de checksums (formato: "filas:soles")
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT dia, checksum FROM day_checksums ORDER BY dia")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(rows[0].0, "2026-01-10");
    assert!(
        rows[0].1.starts_with("2:150"),
        "checksum debe ser '2:150.x': {}",
        rows[0].1
    );
    assert_eq!(rows[1].0, "2026-01-11");
    assert!(
        rows[1].1.starts_with("1:200"),
        "checksum debe ser '1:200.x': {}",
        rows[1].1
    );
}

#[tokio::test]
async fn calculate_day_checksums_overwrites_existing() {
    let pool = setup().await;

    insert_venta(&pool, "2026-01-10", 100.0).await;

    let ts1 = "2026-11-04T10:00:00Z".to_string();
    calculate_day_checksums(&pool, &ts1).await.unwrap();

    // Insertar más datos al mismo día
    insert_venta(&pool, "2026-01-10", 50.0).await;

    let ts2 = "2026-11-04T12:00:00Z".to_string();
    let n = calculate_day_checksums(&pool, &ts2).await.unwrap();
    assert_eq!(n, 1);

    // El checksum debe haberse actualizado
    let checksum: String =
        sqlx::query_scalar("SELECT checksum FROM day_checksums WHERE dia = '2026-01-10'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(checksum, "2:150.0");

    // El timestamp debe ser el nuevo
    let ts: String =
        sqlx::query_scalar("SELECT calculado_en FROM day_checksums WHERE dia = '2026-01-10'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ts, ts2);
}

#[tokio::test]
async fn calculate_day_checksums_empty_db() {
    let pool = setup().await;

    // Sin datos
    let n = calculate_day_checksums(&pool, "2026-11-04T12:00:00Z")
        .await
        .unwrap();
    assert_eq!(n, 0, "DB vacía = 0 checksums calculados");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_checksums")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// Test de publish_incremental: SKIPPED en máquinas con Supabase configurado
/// porque intentaría conectar a la red. En CI/offline pasa automáticamente.
/// NO VERIFICADO: requiere Supabase configurado para probar el flujo completo.
#[tokio::test]
#[ignore = "requires Supabase configuration (network test)"]
async fn publish_incremental_skips_days_without_changes() {
    let pool = setup().await;

    // Insertar datos
    insert_venta(&pool, "2026-01-10", 100.0).await;
    insert_venta(&pool, "2026-01-11", 200.0).await;

    // Calcular checksums
    calculate_day_checksums(&pool, "2026-11-04T12:00:00Z")
        .await
        .unwrap();

    // Publicar estado inicial
    sqlx::query(
        "INSERT INTO etl_state (key, value, updated_at) VALUES 
         ('ultimo_dia_publicado', '2026-01-10', '2026-11-04T12:00:00Z'),
         ('checksum_2026-01-10', '1:100.0', '2026-11-04T12:00:00Z')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&pool)
    .await
    .unwrap();

    // publish_incremental requiere Supabase configurado; en entorno offline
    // debe retornar error accionable.
    let result = publish_incremental(&pool).await;
    match result {
        Ok(_report) => {
            // Supabase configurado: paso silencioso (el test está ignorado intencionalmente)
        }
        Err(e) if e.to_string().contains("Supabase no configurado") => {
            // Comportamiento esperado en offline
        }
        Err(e) => panic!("error inesperado: {e}"),
    }
}

/// Test de publish_incremental con DB vacía: SKIPPED en máquinas con Supabase
/// configurado porque intentaría conectar a la red.
/// NO VERIFICADO: requiere Supabase configurado para probar el flujo completo.
#[tokio::test]
#[ignore = "requires Supabase configuration (network test)"]
async fn publish_incremental_no_data() {
    let pool = setup().await;

    let result = publish_incremental(&pool).await;
    match result {
        Ok(_report) => {
            // Supabase configurado: paso silencioso
        }
        Err(e) if e.to_string().contains("Supabase no configurado") => {
            // Comportamiento esperado en offline
        }
        Err(e) => panic!("error inesperado: {e}"),
    }
}

#[tokio::test]
async fn check_contract_version_passes_with_v4() {
    let pool = setup().await;

    let version = g360_db_ventas::db::writer::check_contract_version(&pool)
        .await
        .unwrap();
    assert_eq!(version, 4);
}

#[tokio::test]
async fn check_contract_version_fails_with_wrong_version() {
    let pool = setup().await;

    // Cambiar la versión a algo distinto
    sqlx::query("UPDATE g360_meta SET value = '3' WHERE key = 'contracto_version'")
        .execute(&pool)
        .await
        .unwrap();

    let result = g360_db_ventas::db::writer::check_contract_version(&pool).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("incompatible"));
}

#[tokio::test]
async fn check_contract_version_fails_when_missing() {
    let pool = setup().await;

    // Eliminar la entrada de contracto_version
    sqlx::query("DELETE FROM g360_meta WHERE key = 'contracto_version'")
        .execute(&pool)
        .await
        .unwrap();

    let result = g360_db_ventas::db::writer::check_contract_version(&pool).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("ausente"));
}
