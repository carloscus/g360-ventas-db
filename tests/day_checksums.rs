// Tests para day_checksums y versionado de contrato.
use g360_db_ventas::db::{writer, schema};
use sqlx::sqlite::SqlitePoolOptions;

async fn setup_test_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    // Crear tablas basicas
    sqlx::query(schema::CREATE_TABLE_SQL)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(schema::CREATE_AUDIT_TABLES_SQL)
        .execute(&pool)
        .await
        .unwrap();
    for idx in schema::CREATE_INDEXES_SQL {
        sqlx::query(idx).execute(&pool).await.unwrap();
    }
    sqlx::query(schema::CREATE_DIM_TABLES_SQL)
        .execute(&pool)
        .await
        .unwrap();
    // Sembrar meta
    sqlx::query(r#"
        INSERT OR REPLACE INTO g360_meta (key, value, actualizado_en)
        VALUES
            ('contracto_version', cast(? as text), datetime('now')),
            ('dim_schema_version', cast(? as text), datetime('now')),
            ('normalizacion_version', cast(? as text), datetime('now'))
    "#)
    .bind(schema::CONTRACTO_VERSION)
    .bind(schema::DIM_SCHEMA_VERSION)
    .bind(schema::NORMALIZACION_VERSION)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn test_day_checksums_calcula_y_persiste() {
    let pool = setup_test_pool().await;
    // Insertar filas de prueba con fechas fijas (no usar now() porque CHECK
    // constraint usa date('now') que no es determinista en SQLite in-memory)
    let today = "2024-06-15".to_string();
    let ayer = "2024-06-14".to_string();
    sqlx::query(
        "INSERT INTO ventas (id_articulo, id_linea, id_vendedor, id_cliente, fecha_orig, mes_ref, anho, mes, \
         tpo_doc, serie_doc, nro_doc, cantidad, soles, moneda, tipo_operacion, folio_unico) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'F01', '204', '111', 10, 100.50, 'Soles', 'venta', 'F01/204/111')"
    )
    .bind("SKU_A")
    .bind("MA")
    .bind("100")
    .bind("CLI_001")
    .bind(&today)
    .bind(&today[..7])
    .bind(today[..4].parse::<i32>().unwrap())
    .bind(today[5..7].parse::<i32>().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ventas (id_articulo, id_linea, id_vendedor, id_cliente, fecha_orig, mes_ref, anho, mes, \
         tpo_doc, serie_doc, nro_doc, cantidad, soles, moneda, tipo_operacion, folio_unico) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'F01', '204', '222', 5, 50.00, 'Soles', 'venta', 'F01/204/222')"
    )
    .bind("SKU_B")
    .bind("01")
    .bind("101")
    .bind("CLI_002")
    .bind(&ayer)
    .bind(&ayer[..7])
    .bind(ayer[..4].parse::<i32>().unwrap())
    .bind(ayer[5..7].parse::<i32>().unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let n = writer::calculate_day_checksums(&pool, &ts).await.unwrap();
    assert_eq!(n, 2);

    // Verificar que los checksums persistieron
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_checksums")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn test_contracto_version_constant() {
    assert_eq!(schema::CONTRACTO_VERSION, 4);
    assert_eq!(schema::NORMALIZACION_VERSION, 2);
    assert_eq!(schema::DIM_SCHEMA_VERSION, 3);
}

#[tokio::test]
async fn test_check_contract_version_pasa() {
    let pool = setup_test_pool().await;
    // La version deberia ser compatible porque sembramos CONTRACTO_VERSION=4
    assert!(writer::check_contract_version(&pool).await.is_ok());
}

#[tokio::test]
async fn test_check_contract_version_falla_con_mismatch() {
    let pool = setup_test_pool().await;
    // Cambiar manualmente la version en g360_meta para simular mismatch
    sqlx::query("UPDATE g360_meta SET value = '3' WHERE key = 'contracto_version'")
        .execute(&pool)
        .await
        .unwrap();
    // Deberia fallar porque esperamos v4
    let result = writer::check_contract_version(&pool).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("incompatibles"));
}
