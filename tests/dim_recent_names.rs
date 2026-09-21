// Dimensiones: la CLAVE es el id estable (id_cliente/id_articulo/id_linea/
// id_vendedor) y los ATRIBUTOS (nombre, RUC, ubicacion, catalogo) se toman de
// la fila MAS RECIENTE. Si el ERP renombra una entidad entre capturas, la
// dimension debe mostrar el nombre VIGENTE, no el MAX() alfabetico.
//
// Este test construye una BD en memoria con nombres viejos que "ganarian" el
// MAX() (empiezan con ZZZ) y confirma que gana el nombre mas reciente.
use g360_db_ventas::db::schema::{
    CREATE_DIM_TABLES_SQL, CREATE_TABLE_SQL, CREATE_VIEWS_SQL, DIM_SCHEMA_VERSION,
};
use g360_db_ventas::db::writer::{ensure_dims_fresh, refresh_dim_tables};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

async fn setup() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("conectar sqlite::memory:");
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await.unwrap();
    sqlx::query(CREATE_DIM_TABLES_SQL)
        .execute(&pool)
        .await
        .unwrap();
    pool
}

/// Inserta una venta minima con todos los atributos de las 4 entidades.
#[allow(clippy::too_many_arguments)]
async fn insert(
    pool: &SqlitePool,
    fecha: &str,
    mes_ref: &str,
    id_cliente: &str,
    nom_cliente: &str,
    id_articulo: &str,
    nom_articulo: &str,
    id_linea: &str,
    nom_linea: &str,
    id_vendedor: &str,
    nom_vendedor: &str,
    soles: f64,
    cantidad: f64,
) {
    sqlx::query(
        r#"INSERT INTO ventas
           (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
            id_cliente, doc_cliente, nom_cliente, id_vendedor, nom_vendedor,
            tpo_doc, serie_doc, nro_doc, cantidad, soles, anho, mes,
            fecha_orig, mes_ref, tipo_operacion)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'F01', '201', '1',
                   ?, ?, 2025, 1, ?, ?, 'venta')"#,
    )
    .bind(id_articulo)
    .bind(id_articulo) // original_sku
    .bind(nom_articulo)
    .bind(id_linea)
    .bind(nom_linea)
    .bind(id_cliente)
    .bind(format!("RUC-{id_cliente}"))
    .bind(nom_cliente)
    .bind(id_vendedor)
    .bind(nom_vendedor)
    .bind(cantidad)
    .bind(soles)
    .bind(fecha)
    .bind(mes_ref)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn dims_muestran_el_nombre_mas_reciente() {
    let pool = setup().await;

    // Fila ANTIGUA (2025-01): nombres viejos que ganarian el MAX() por empezar con ZZZ.
    insert(
        &pool, "2025-01-10", "2025-01",
        "C1", "ZZZ CLIENTE VIEJO",
        "S1", "ZZZ PRODUCTO VIEJO",
        "L1", "ZZZ LINEA VIEJA",
        "V1", "ZZZ VENDEDOR VIEJO",
        100.0, 10.0,
    )
    .await;

    // Fila NUEVA (2026-01): mismos ids, nombres renombrados.
    insert(
        &pool, "2026-01-10", "2026-01",
        "C1", "CLIENTE NUEVO",
        "S1", "PRODUCTO NUEVO",
        "L1", "LINEA NUEVA",
        "V1", "VENDEDOR NUEVO",
        200.0, 20.0,
    )
    .await;

    refresh_dim_tables(&pool).await.expect("refresh dims");

    // ── Cliente ──
    let r = sqlx::query("SELECT nom_cliente, doc_cliente, n_ventas, total_soles FROM dim_cliente WHERE id_cliente='C1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(r.get::<String, _>("nom_cliente"), "CLIENTE NUEVO",
        "dim_cliente debe mostrar el nombre mas reciente");
    assert_eq!(r.get::<String, _>("doc_cliente"), "RUC-C1");
    assert_eq!(r.get::<i64, _>("n_ventas"), 2);
    assert!((r.get::<f64, _>("total_soles") - 300.0).abs() < 0.01);

    // ── Vendedor ──
    let r = sqlx::query("SELECT nom_vendedor, n_ventas FROM dim_vendedor WHERE id_vendedor='V1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(r.get::<String, _>("nom_vendedor"), "VENDEDOR NUEVO");
    assert_eq!(r.get::<i64, _>("n_ventas"), 2);

    // ── Articulo (SKU estable, nombre + jerarquia de linea vigentes) ──
    let r = sqlx::query(
        "SELECT nom_articulo, id_linea, nom_linea, n_ventas, total_cantidad, total_soles, precio_promedio
         FROM dim_articulo WHERE id_articulo='S1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(r.get::<String, _>("nom_articulo"), "PRODUCTO NUEVO",
        "dim_articulo debe mostrar el nombre mas reciente");
    assert_eq!(r.get::<String, _>("id_linea"), "L1");
    assert_eq!(r.get::<String, _>("nom_linea"), "LINEA NUEVA");
    assert_eq!(r.get::<i64, _>("n_ventas"), 2);
    assert!((r.get::<f64, _>("total_cantidad") - 30.0).abs() < 0.01);
    assert!((r.get::<f64, _>("total_soles") - 300.0).abs() < 0.01);
    assert!((r.get::<f64, _>("precio_promedio") - 10.0).abs() < 0.01);

    // ── Linea ──
    let r = sqlx::query("SELECT nom_linea, n_ventas, total_soles FROM dim_linea WHERE id_linea='L1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(r.get::<String, _>("nom_linea"), "LINEA NUEVA",
        "dim_linea debe mostrar el nombre mas reciente");
    assert_eq!(r.get::<i64, _>("n_ventas"), 2);
    assert!((r.get::<f64, _>("total_soles") - 300.0).abs() < 0.01);
}

#[tokio::test]
async fn ensure_dims_fresh_versiona_y_no_reconstruye_al_dia() {
    let pool = setup().await;
    insert(
        &pool, "2026-01-10", "2026-01",
        "C9", "CLIENTE X", "S9", "PRODUCTO X",
        "L9", "LINEA X", "V9", "VENDEDOR X",
        50.0, 5.0,
    )
    .await;

    // Primera vez: dims vacias -> reconstruye y marca la version.
    let n1 = ensure_dims_fresh(&pool).await.expect("ensure 1");
    assert!(n1 > 0, "debe reconstruir cuando estan vacias");
    let ver: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&pool).await.unwrap();
    assert_eq!(ver, DIM_SCHEMA_VERSION, "debe marcar la version del esquema");

    // Segunda vez: ya al dia -> no reconstruye (barato).
    let n2 = ensure_dims_fresh(&pool).await.expect("ensure 2");
    assert_eq!(n2, 0, "no debe reconstruir si ya esta al dia");

    // Si bajamos la version (simula BD antigua), debe reconstruir una vez.
    sqlx::query("PRAGMA user_version = 1").execute(&pool).await.unwrap();
    let n3 = ensure_dims_fresh(&pool).await.expect("ensure 3");
    assert!(n3 > 0, "debe reconstruir si user_version < DIM_SCHEMA_VERSION");
    let ver: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&pool).await.unwrap();
    assert_eq!(ver, DIM_SCHEMA_VERSION);
}

#[tokio::test]
async fn vistas_se_reemplazan_aunque_existan() {
    let pool = setup().await;

    // Simula una BD heredada con la definicion VIEJA de vw_dim_linea (MAX).
    sqlx::query(
        "CREATE VIEW vw_dim_linea AS
         SELECT id_linea, MAX(nom_linea) AS nom_linea
         FROM ventas WHERE id_linea != '' GROUP BY id_linea",
    )
    .execute(&pool)
    .await
    .unwrap();

    let vieja: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='view' AND name='vw_dim_linea'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(vieja.contains("MAX("), "precondicion: la vista vieja usa MAX()");

    // Re-aplicar el esquema de vistas debe reemplazarla (DROP + CREATE).
    for view in CREATE_VIEWS_SQL {
        sqlx::query(view).execute(&pool).await.expect("crear vistas");
    }

    let nueva: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='view' AND name='vw_dim_linea'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        nueva.contains("ROW_NUMBER"),
        "vw_dim_linea debe haber sido reemplazada por la version con ROW_NUMBER"
    );
}
