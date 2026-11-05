// vw_historial_articulo: evolucion mensual por SKU de nombre, linea y PRECIO
// UNITARIO, con comparacion contra el mes anterior. Verifica que un renombre y
// un cambio de linea se detectan (cambio_nombre/cambio_linea) y que el precio
// unitario y su variacion % son correctos.
use g360_db_ventas::db::schema::{CREATE_TABLE_SQL, CREATE_VIEWS_SQL};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

async fn setup() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await.unwrap();
    for v in CREATE_VIEWS_SQL {
        sqlx::query(v).execute(&pool).await.expect("crear vista");
    }
    pool
}

async fn venta(
    pool: &SqlitePool,
    fecha: &str,
    sku: &str,
    nom: &str,
    linea: &str,
    cant: f64,
    soles: f64,
) {
    sqlx::query(
        r#"INSERT INTO ventas
           (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
            id_cliente, nom_cliente, id_vendedor, nom_vendedor, tpo_doc, cantidad, soles,
            anho, mes, fecha_orig, mes_ref, tipo_operacion)
           VALUES (?,?,?,?,?, 'C1','CLIENTE','V1','VENDEDOR','F01',?,?, 2024,1,?,?, 'venta')"#,
    )
    .bind(sku)
    .bind(sku)
    .bind(nom)
    .bind(linea)
    .bind(format!("LINEA {linea}"))
    .bind(cant)
    .bind(soles)
    .bind(fecha)
    .bind(&fecha[..7])
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn historial_muestra_renombre_cambio_linea_y_precio() {
    let pool = setup().await;

    // 2023-05: MORRAL CLASICO (L78) a S/ 10.00
    venta(&pool, "2023-05-10", "55555", "MORRAL CLASICO", "78", 10.0, 100.0).await;
    // 2024-03: lo renombran a MORRAL PREMIUM, lo mueven a L76 y suben a S/ 12.00
    venta(&pool, "2024-03-15", "55555", "MORRAL PREMIUM", "76", 8.0, 96.0).await;
    // 2024-04: mismo nombre/linea, precio S/ 15.00
    venta(&pool, "2024-04-20", "55555", "MORRAL PREMIUM", "76", 5.0, 75.0).await;

    let rows = sqlx::query(
        "SELECT mes_ref, nom_articulo, id_linea, p_u_venta, p_u_venta_anterior,
                cambio_nombre, cambio_linea, variacion_precio_pct
         FROM vw_historial_articulo WHERE id_articulo='55555' ORDER BY mes_ref",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 3, "3 meses de historia");

    // 2023-05 (primero: sin mes anterior)
    let r0 = &rows[0];
    assert_eq!(r0.get::<String, _>("mes_ref"), "2023-05");
    assert_eq!(r0.get::<String, _>("nom_articulo"), "MORRAL CLASICO");
    assert!((r0.get::<f64, _>("p_u_venta") - 10.0).abs() < 0.001);
    assert!(r0.get::<Option<f64>, _>("p_u_venta_anterior").is_none());
    assert_eq!(r0.get::<i64, _>("cambio_nombre"), 0);
    assert_eq!(r0.get::<i64, _>("cambio_linea"), 0);

    // 2024-03 (renombre + cambio de linea + subida de precio)
    let r1 = &rows[1];
    assert_eq!(r1.get::<String, _>("mes_ref"), "2024-03");
    assert_eq!(r1.get::<String, _>("nom_articulo"), "MORRAL PREMIUM");
    assert_eq!(r1.get::<i64, _>("cambio_nombre"), 1, "debe marcar el renombre");
    assert_eq!(r1.get::<i64, _>("cambio_linea"), 1, "debe marcar el cambio de linea");
    assert!((r1.get::<f64, _>("p_u_venta") - 12.0).abs() < 0.001);
    assert!((r1.get::<f64, _>("p_u_venta_anterior") - 10.0).abs() < 0.001);
    assert!((r1.get::<f64, _>("variacion_precio_pct") - 20.0).abs() < 0.01,
        "10 -> 12 = +20%");

    // 2024-04 (solo cambio de precio)
    let r2 = &rows[2];
    assert_eq!(r2.get::<i64, _>("cambio_nombre"), 0);
    assert_eq!(r2.get::<i64, _>("cambio_linea"), 0);
    assert!((r2.get::<f64, _>("p_u_venta") - 15.0).abs() < 0.001);
    assert!((r2.get::<f64, _>("variacion_precio_pct") - 25.0).abs() < 0.01,
        "12 -> 15 = +25%");
}

#[tokio::test]
async fn historial_no_contamina_precio_con_devoluciones() {
    let pool = setup().await;

    // Venta 10 und a S/ 10 = 100
    venta(&pool, "2024-01-10", "77777", "PRODUCTO", "01", 10.0, 100.0).await;
    // Devolucion de 2 und (-20): no debe diluir el precio de venta
    sqlx::query(
        r#"INSERT INTO ventas
           (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
            id_cliente, nom_cliente, id_vendedor, nom_vendedor, tpo_doc, cantidad, soles,
            anho, mes, fecha_orig, mes_ref, tipo_operacion)
           VALUES ('77777','77777','PRODUCTO','01','LINEA 01','C1','CLIENTE','V1','VENDEDOR',
                   'NCR', -2, -20, 2024, 1, '2024-01-20', '2024-01', 'devolucion')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let r = sqlx::query(
        "SELECT p_u_venta, p_u_neto, vendido, devuelto, cantidad_neta
         FROM vw_historial_articulo WHERE id_articulo='77777' AND mes_ref='2024-01'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // p_u_venta solo sobre ventas: 100/10 = 10
    assert!((r.get::<f64, _>("p_u_venta") - 10.0).abs() < 0.001,
        "el precio de venta no debe diluirse por la devolucion");
    // soles_netos = 100 + (-20) = 80 ; cantidad_neta = 10 - 2 = 8 -> p_u_neto = 10
    assert!((r.get::<f64, _>("p_u_neto") - 10.0).abs() < 0.001);
    assert!((r.get::<f64, _>("vendido") - 10.0).abs() < 0.001);
    // devuelto se expone como MAGNITUD positiva (robusto a la convencion de signos)
    assert!((r.get::<f64, _>("devuelto") - 2.0).abs() < 0.001);
    assert!((r.get::<f64, _>("cantidad_neta") - 8.0).abs() < 0.001);
}
