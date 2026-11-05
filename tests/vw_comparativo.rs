// vw_comparativo_ventas / vw_comparativo_linea: comparacion de cantidad y monto
// contra el año anterior (YoY) y el mes anterior (MoM), con variacion %.
// Verifica el caso clave: cuando faltan meses, YoY debe resolverse por fecha
// exacta (no por LAG), y el rollup de linea no debe inflarse con SKUs nuevos.
use g360_db_ventas::db::schema::{CREATE_DIM_TABLES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL};
use g360_db_ventas::db::writer::refresh_dim_tables;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

async fn setup() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await.unwrap();
    sqlx::query(CREATE_DIM_TABLES_SQL)
        .execute(&pool)
        .await
        .unwrap();
    for v in CREATE_VIEWS_SQL {
        sqlx::query(v).execute(&pool).await.expect("crear vista");
    }
    pool
}

/// Reconstruye fact_venta_mes (y dims) tras insertar ventas.
async fn refresh(pool: &SqlitePool) {
    refresh_dim_tables(pool).await.expect("refresh dims");
}

async fn venta(pool: &SqlitePool, mes_ref: &str, linea: &str, sku: &str, cant: f64, soles: f64) {
    sqlx::query(
        r#"INSERT INTO ventas
           (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
            id_cliente, nom_cliente, id_vendedor, nom_vendedor, tpo_doc, serie_doc, nro_doc,
            cantidad, soles, anho, mes, fecha_orig, mes_ref, tipo_operacion, folio_unico)
           VALUES (?,?,?,?,?, 'C1','CLIENTE','V1','VENDEDOR','F01','001','0001',?,?,
                   2025,1,?,?,'venta','F01/001/0001')"#,
    )
    .bind(sku)
    .bind(sku)
    .bind(format!("PRODUCTO {sku}"))
    .bind(linea)
    .bind(format!("LINEA {linea}"))
    .bind(cant)
    .bind(soles)
    .bind(format!("{mes_ref}-15"))
    .bind(mes_ref)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn comparativo_yoy_y_mom() {
    let pool = setup().await;

    // SKU A (linea 01): 2024-01, 2024-02, 2025-01
    venta(&pool, "2024-01", "01", "A", 10.0, 100.0).await;
    venta(&pool, "2024-02", "01", "A", 20.0, 200.0).await;
    venta(&pool, "2025-01", "01", "A", 15.0, 180.0).await;
    refresh(&pool).await;

    // 2025-01: YoY contra 2024-01 (10 und / 100 S/)
    let r = sqlx::query(
        "SELECT cantidad, soles, cantidad_anio_ant, soles_anio_ant,
                var_cantidad_pct, var_soles_pct, cantidad_mes_ant
         FROM vw_comparativo_ventas
         WHERE id_articulo='A' AND mes_ref='2025-01'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((r.get::<f64, _>("cantidad") - 15.0).abs() < 0.001);
    assert!((r.get::<f64, _>("soles") - 180.0).abs() < 0.001);
    assert!((r.get::<f64, _>("cantidad_anio_ant") - 10.0).abs() < 0.001);
    assert!((r.get::<f64, _>("soles_anio_ant") - 100.0).abs() < 0.001);
    assert!((r.get::<f64, _>("var_cantidad_pct") - 50.0).abs() < 0.01, "10 -> 15 = +50%");
    assert!((r.get::<f64, _>("var_soles_pct") - 80.0).abs() < 0.01, "100 -> 180 = +80%");
    // No hay 2024-12 -> MoM nulo
    assert!(r.get::<Option<f64>, _>("cantidad_mes_ant").is_none());

    // 2024-02: MoM contra 2024-01 (10 -> 20 = +100%)
    let r = sqlx::query(
        "SELECT cantidad_mes_ant, var_cantidad_mom_pct
         FROM vw_comparativo_ventas WHERE id_articulo='A' AND mes_ref='2024-02'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((r.get::<f64, _>("cantidad_mes_ant") - 10.0).abs() < 0.001);
    assert!((r.get::<f64, _>("var_cantidad_mom_pct") - 100.0).abs() < 0.01);
}

#[tokio::test]
async fn linea_no_se_infla_con_sku_nuevo() {
    let pool = setup().await;

    // Linea 01: SKU A existia en 2024-01 (10/100) y sigue en 2025-01 (15/180).
    venta(&pool, "2024-01", "01", "A", 10.0, 100.0).await;
    venta(&pool, "2025-01", "01", "A", 15.0, 180.0).await;
    // SKU NUEVO (B) solo en 2025-01: no tiene año anterior.
    venta(&pool, "2025-01", "01", "B", 100.0, 1000.0).await;
    refresh(&pool).await;

    let r = sqlx::query(
        "SELECT cantidad, cantidad_anio_ant, cantidad_comparable, var_cantidad_pct, soles, soles_comparable
         FROM vw_comparativo_linea WHERE id_linea='01' AND mes_ref='2025-01'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Total del mes incluye A+B
    assert!((r.get::<f64, _>("cantidad") - 115.0).abs() < 0.001);
    // YoY disponible solo de A
    assert!((r.get::<f64, _>("cantidad_anio_ant") - 10.0).abs() < 0.001);
    // comparable = solo A (B no tiene año anterior) -> 15
    assert!((r.get::<f64, _>("cantidad_comparable") - 15.0).abs() < 0.001);
    // % peras-con-peras: 10 -> 15 = +50% (NO 10 -> 115)
    assert!((r.get::<f64, _>("var_cantidad_pct") - 50.0).abs() < 0.01,
        "el SKU nuevo no debe inflar la variacion de la linea");
    assert!((r.get::<f64, _>("soles_comparable") - 180.0).abs() < 0.001);
}
