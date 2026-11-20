// Tests P0 de insert_ventas: dedup por fila completa (4-tupla) y modo día/mes
// con guard asimétrico. El riesgo que cubren es la pérdida de líneas legítimas
// multi-artículo (caso BDI): folio_unico identifica el DOCUMENTO, no la fila.
use chrono::NaiveDate;
use g360_db_ventas::db::schema::{CREATE_INDEXES_SQL, CREATE_TABLE_SQL};
use g360_db_ventas::db::writer::{insert_ventas, ModoInsert};
use g360_db_ventas::Venta;

fn venta(folio: &str, articulo: &str, cantidad: f64, soles: f64, dia: u32, mes_ref: &str) -> Venta {
    Venta {
        id_articulo: articulo.into(), original_sku: articulo.into(), nom_articulo: "Art".into(),
        id_linea: "01".into(), nom_linea: "Linea".into(), id_grupo: "".into(), nom_grupo: "".into(),
        id_tipo: "".into(), nom_tipo: "".into(), id_familia: "".into(), nom_familia: "".into(),
        id_cliente: "1".into(), doc_cliente: "".into(), nom_cliente: "Cli".into(),
        tpo_doc: "F01".into(), serie_doc: "201".into(), nro_doc: "1".into(), referencia: "".into(),
        moneda: "Soles".into(), cantidad, cantidad_fae: cantidad, soles, dolares: 0.0,
        precio_unitario: if cantidad != 0.0 { soles / cantidad } else { 0.0 },
        anho: 2026, mes: 11, fecha_orig: NaiveDate::from_ymd_opt(2026, 11, dia).unwrap(),
        fecha_ref: None, fecha_venc: None,
        cod_sucursal: "".into(), nom_sucursal: "".into(), departamento: "".into(),
        provincia: "".into(), distrito: "".into(), id_vendedor: "".into(), nom_vendedor: "".into(),
        id_pedido: "".into(), ord_compra: "".into(), file_source: "test".into(), mes_ref: mes_ref.into(),
        tipo_operacion: "venta".into(), factura_ref_serie: "".into(), factura_ref_nro: "".into(),
        folio_unico: folio.into(), id_ubigeo: "".into(), canal_distribucion: "".into(),
        nom_condicion_pago: "".into(), estado_linea: "".into(), division: "".into(),
        fec_cargo: "".into(), id_guia: "".into(),
    }
}

async fn pool() -> sqlx::SqlitePool {
    let p = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1).connect("sqlite::memory:").await.expect("pool");
    sqlx::query(CREATE_TABLE_SQL).execute(&p).await.unwrap();
    for idx in CREATE_INDEXES_SQL { sqlx::query(idx).execute(&p).await.unwrap(); }
    p
}

async fn count(p: &sqlx::SqlitePool, sql: &str) -> usize {
    sqlx::query_scalar::<_, i64>(sql).fetch_one(p).await.unwrap() as usize
}

#[tokio::test]
async fn dedup_4tupla_conserva_lineas_multi_articulo_y_multi_linea() {
    let p = pool().await;
    let mut vs = vec![
        venta("F/201-1", "SKU-A", 1.0, 10.0, 16, "2026-11"),
        venta("F/201-1", "SKU-B", 2.0, 20.0, 16, "2026-11"), // mismo folio, distinto artículo: válida
        venta("F/201-1", "SKU-A", 3.0, 30.0, 16, "2026-11"), // mismo folio+artículo, distinta cantidad/soles: válida
    ];
    let dup = venta("F/201-1", "SKU-A", 1.0, 10.0, 16, "2026-11"); // 4-tupla exacta repetida
    vs.push(dup);
    let n = insert_ventas(&p, &vs, ModoInsert::Mes, false).await.unwrap();
    assert_eq!(n, 3, "solo se descarta la 4-tupla duplicada; las líneas legítimas se conservan");
    assert_eq!(count(&p, "SELECT COUNT(*) FROM ventas").await, 3);
}

#[tokio::test]
async fn modo_dia_guard_asimetrico_no_vacia_el_dia() {
    let p = pool().await;
    let base: Vec<Venta> = (1..=1001)
        .map(|i| venta(&format!("F/201-{}", i), "SKU-A", 1.0, 10.0, 16, "2026-11"))
        .collect();
    insert_ventas(&p, &base, ModoInsert::Dia, false).await.unwrap();
    assert_eq!(count(&p, "SELECT COUNT(*) FROM ventas").await, 1001);

    // Otro día distinto no debe ser tocado por el reemplazo del 16.
    let otro = venta("F/202-1", "SKU-A", 1.0, 10.0, 17, "2026-11");
    insert_ventas(&p, &[otro], ModoInsert::Dia, false).await.unwrap();

    // <50% => Err y NO se borra nada.
    let corto: Vec<Venta> = (1..=3)
        .map(|i| venta(&format!("G/201-{}", i), "SKU-A", 1.0, 10.0, 16, "2026-11"))
        .collect();
    assert!(insert_ventas(&p, &corto, ModoInsert::Dia, false).await.is_err());
    assert_eq!(count(&p, "SELECT COUNT(*) FROM ventas WHERE fecha_orig = '2026-11-16'").await, 1001);

    // >=50% => reemplaza SOLO el rango del día 16; el día 17 queda intacto.
    let bueno: Vec<Venta> = (1..=600)
        .map(|i| venta(&format!("H/201-{}", i), "SKU-A", 1.0, 10.0, 16, "2026-11"))
        .collect();
    insert_ventas(&p, &bueno, ModoInsert::Dia, false).await.unwrap();
    assert_eq!(count(&p, "SELECT COUNT(*) FROM ventas WHERE fecha_orig = '2026-11-16'").await, 600);
    assert_eq!(count(&p, "SELECT COUNT(*) FROM ventas WHERE fecha_orig = '2026-11-17'").await, 1);
}

#[tokio::test]
async fn modo_mes_guard_asimetrico() {
    let p = pool().await;
    let base: Vec<Venta> = (1..=1001)
        .map(|i| venta(&format!("F/301-{}", i), "SKU-A", 1.0, 10.0, 15, "2026-10"))
        .collect();
    insert_ventas(&p, &base, ModoInsert::Mes, false).await.unwrap();
    let corto: Vec<Venta> = (1..=2)
        .map(|i| venta(&format!("G/301-{}", i), "SKU-A", 1.0, 10.0, 15, "2026-10"))
        .collect();
    assert!(insert_ventas(&p, &corto, ModoInsert::Mes, false).await.is_err());
    assert_eq!(count(&p, "SELECT COUNT(*) FROM ventas WHERE mes_ref = '2026-10'").await, 1001);
}

// El abort por sub-captura BDI es una función pura compartida por los dos
// transportes. Se testea aquí para que la garantía no dependa de red ni del
// transporte elegido en la configuración.
#[test]
fn validar_no_perdida_split_aborta_cuando_merged_es_menor_que_referencia() {
    use g360_db_ventas::capture::validar_no_perdida_split;

    // Caso real del defecto: 250 filas en el cartucho vs 590 en el ERP.
    assert!(validar_no_perdida_split("2026-11-05", 250, 590).is_err());
    // Igual o más filas: válido.
    assert!(validar_no_perdida_split("2026-11-05", 590, 590).is_ok());
    assert!(validar_no_perdida_split("2026-11-05", 600, 590).is_ok());
    // Sin referencia no se puede comparar; el flujo real ya abortó antes si la
    // descarga de referencia falló, así que aquí no se lanza un falso abort.
    assert!(validar_no_perdida_split("2026-11-05", 100, 0).is_ok());
}

#[test]
fn validar_invariante_checksums_rechaza_suma_discrepante() {
    use g360_db_ventas::db::writer::validar_invariante_checksums;

    assert!(validar_invariante_checksums(2_834_124, 2_834_124).is_ok());
    // Suma de day_checksums menor que COUNT(ventas): cartucho NO distribuible.
    assert!(validar_invariante_checksums(250, 590).is_err());
    assert!(validar_invariante_checksums(600, 590).is_err());
}
