pub const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS ventas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    id_articulo TEXT NOT NULL, original_sku TEXT, nom_articulo TEXT,
    id_linea TEXT NOT NULL, nom_linea TEXT,
    id_grupo TEXT, nom_grupo TEXT,
    id_tipo TEXT, nom_tipo TEXT,
    id_familia TEXT, nom_familia TEXT,
    id_cliente TEXT NOT NULL, doc_cliente TEXT, nom_cliente TEXT,
    tpo_doc TEXT NOT NULL, serie_doc TEXT, nro_doc TEXT,
    referencia TEXT, moneda TEXT DEFAULT 'Soles',
    cantidad REAL NOT NULL, soles REAL NOT NULL, dolares REAL, precio_unitario REAL,
    cantidad_fae REAL,
    anho INTEGER NOT NULL, mes INTEGER NOT NULL,
    fecha_orig TEXT NOT NULL,
    fecha_ref TEXT, fecha_venc TEXT,
    cod_sucursal TEXT, nom_sucursal TEXT,
    departamento TEXT, provincia TEXT,     distrito TEXT,
    id_vendedor TEXT, nom_vendedor TEXT,
    id_pedido TEXT,
    file_source TEXT, mes_ref TEXT NOT NULL,
    capturado_en TEXT DEFAULT (datetime('now')),
    tipo_operacion TEXT DEFAULT 'venta',
    factura_ref_serie TEXT,
    factura_ref_nro TEXT,
    folio_unico TEXT
)";

pub const CREATE_INDEXES_SQL: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_venta_mes ON ventas(mes_ref);",
    "CREATE INDEX IF NOT EXISTS idx_venta_cliente ON ventas(id_cliente);",
    "CREATE INDEX IF NOT EXISTS idx_venta_doc_cliente ON ventas(doc_cliente);",
    "CREATE INDEX IF NOT EXISTS idx_venta_sku ON ventas(id_articulo);",
    "CREATE INDEX IF NOT EXISTS idx_venta_orig_sku ON ventas(original_sku);",
    "CREATE INDEX IF NOT EXISTS idx_venta_linea ON ventas(id_linea);",
    "CREATE INDEX IF NOT EXISTS idx_venta_doc ON ventas(tpo_doc, serie_doc, nro_doc);",
    "CREATE INDEX IF NOT EXISTS idx_venta_ref ON ventas(referencia);",
    "CREATE INDEX IF NOT EXISTS idx_venta_tipo_op ON ventas(tipo_operacion);",
    "CREATE INDEX IF NOT EXISTS idx_venta_fact_ref ON ventas(factura_ref_serie, factura_ref_nro);",
    "CREATE INDEX IF NOT EXISTS idx_venta_fecha ON ventas(fecha_orig);",
];

/// Vistas listas para consumir por CRM / dashboard / plataforma de devoluciones.
/// Son VIEWS virtuales (no ocupan almacenamiento extra; se calculan al consultar).
pub const CREATE_VIEWS_SQL: &[&str] = &[
    // ─── Dimensiones ─────────────────────────────────────────────────
    // Clientes unicos (fuente de verdad: cada cliente con su ultimo nombre conocido)
    r#"
    CREATE VIEW IF NOT EXISTS vw_dim_cliente AS
    SELECT id_cliente, doc_cliente, MAX(nom_cliente) AS nom_cliente,
           MAX(departamento) AS departamento, MAX(provincia) AS provincia, MAX(distrito) AS distrito
    FROM ventas WHERE id_cliente != '' GROUP BY id_cliente
    "#,
    // Articulos unicos: todas las dimensiones de producto
    r#"
    CREATE VIEW IF NOT EXISTS vw_dim_articulo AS
    SELECT id_articulo, original_sku, MAX(nom_articulo) AS nom_articulo,
           MAX(id_linea) AS id_linea, MAX(nom_linea) AS nom_linea,
           MAX(id_grupo) AS id_grupo, MAX(nom_grupo) AS nom_grupo,
           MAX(id_tipo) AS id_tipo, MAX(nom_tipo) AS nom_tipo,
           MAX(id_familia) AS id_familia, MAX(nom_familia) AS nom_familia
    FROM ventas WHERE id_articulo != '' GROUP BY id_articulo
    "#,
    // Lineas / categorias del ERP (jerarquia de producto)
    r#"
    CREATE VIEW IF NOT EXISTS vw_dim_linea AS
    SELECT id_linea, MAX(nom_linea) AS nom_linea,
           MAX(id_grupo) AS id_grupo, MAX(nom_grupo) AS nom_grupo,
           MAX(id_familia) AS id_familia, MAX(nom_familia) AS nom_familia
    FROM ventas WHERE id_linea != '' GROUP BY id_linea
    "#,

    // ─── Hechos / analisis ───────────────────────────────────────────
    // Cabecera por documento (una fila por factura/NC/ND — para CRM y conciliacion)
    r#"
    CREATE VIEW IF NOT EXISTS vw_documento AS
    SELECT
        tpo_doc, serie_doc, nro_doc, mes_ref, fecha_orig,
        id_cliente, doc_cliente, nom_cliente,
        COUNT(*) AS n_lineas,
        SUM(cantidad) AS cantidad,
        ROUND(SUM(soles), 2) AS total_soles,
        ROUND(SUM(dolares), 2) AS total_dolares,
        COALESCE(factura_ref_serie,'') || '/' || COALESCE(factura_ref_nro,'') AS referencia_factura
    FROM ventas
    GROUP BY tpo_doc, serie_doc, nro_doc
    "#,
    // Devoluciones/ajustes por factura origen (plataforma de devoluciones y cantidades verdaderas)
    r#"
    CREATE VIEW IF NOT EXISTS vw_devoluciones AS
    SELECT
        n.mes_ref,
        n.tpo_doc AS nc_tpo, n.serie_doc AS nc_serie, n.nro_doc AS nc_nro, n.fecha_orig AS nc_fecha,
        n.id_articulo, n.nom_articulo, n.id_linea, n.nom_linea,
        n.cantidad AS cant_devuelta, n.cantidad_fae AS fae_base, n.soles AS soles_devueltos,
        n.tipo_operacion,
        f.tpo_doc AS fac_tpo, f.serie_doc AS fac_serie, f.nro_doc AS fac_nro,
        f.fecha_orig AS fac_fecha, f.precio_unitario AS precio_original,
        ROUND(n.soles / NULLIF(n.cantidad_fae, 0), 4) AS descuento_unit,
        f.mes_ref AS fac_mes
    FROM ventas n
    LEFT JOIN ventas f
        ON f.serie_doc = n.factura_ref_serie AND f.nro_doc = n.factura_ref_nro
        AND (f.tpo_doc LIKE 'F01%' OR f.tpo_doc = 'F01')
    WHERE (n.tpo_doc LIKE '%NCR%' OR n.tpo_doc LIKE '%NDB%')
    "#,
    // Ventas netas por articulo x mes: vendido - devuelto = cantidad verdadera (dashboard/inventario)
    r#"
    CREATE VIEW IF NOT EXISTS vw_venta_neta_producto AS
    SELECT
        v.id_articulo, v.nom_articulo, v.id_linea, v.nom_linea, v.mes_ref,
        SUM(CASE WHEN v.tipo_operacion = 'venta' THEN v.cantidad ELSE 0 END) AS vendido,
        SUM(CASE WHEN v.tipo_operacion = 'devolucion' THEN v.cantidad ELSE 0 END) AS devuelto,
        SUM(v.cantidad) AS cantidad_neta,
        ROUND(SUM(CASE WHEN v.tipo_operacion = 'venta' THEN v.soles ELSE 0 END), 2) AS soles_vendidos,
        ROUND(SUM(v.soles), 2) AS soles_netos,
        ROUND(SUM(v.soles) / NULLIF(SUM(v.cantidad), 0), 4) AS p_u_neto
    FROM ventas v
    GROUP BY v.id_articulo, v.mes_ref
    "#,
];
