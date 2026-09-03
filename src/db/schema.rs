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

// Tablas de auditoría e integridad
pub const CREATE_AUDIT_TABLES_SQL: &str = "
-- Log de sincronizaciones
CREATE TABLE IF NOT EXISTS sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tipo TEXT NOT NULL DEFAULT 'upload',
    estado TEXT NOT NULL DEFAULT 'pending',
    filas_solicitadas INTEGER DEFAULT 0,
    filas_subidas INTEGER DEFAULT 0,
    filas_limpiadas INTEGER DEFAULT 0,
    duracion_segundos REAL,
    error_message TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT
);

-- Checksums mensuales para detectar cambios
CREATE TABLE IF NOT EXISTS mes_checksums (
    mes_ref TEXT NOT NULL,
    checksum TEXT NOT NULL,
    total_filas INTEGER NOT NULL,
    total_soles REAL NOT NULL,
    total_cantidad REAL,
    calculado_en TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (mes_ref)
);

-- Auditoría de cambios
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tabla TEXT NOT NULL,
    operacion TEXT NOT NULL,
    folio_unico TEXT,
    id_articulo TEXT,
    filas_afectadas INTEGER DEFAULT 1,
    detalle TEXT,
    creado_en TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Indices
CREATE INDEX IF NOT EXISTS idx_sync_log_fecha ON sync_log(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_sync_log_tipo ON sync_log(tipo);
CREATE INDEX IF NOT EXISTS idx_audit_log_folio ON audit_log(folio_unico);
CREATE INDEX IF NOT EXISTS idx_audit_log_fecha ON audit_log(creado_en DESC);
";

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
    "CREATE INDEX IF NOT EXISTS idx_venta_soles ON ventas(soles);",
    "CREATE INDEX IF NOT EXISTS idx_venta_fecha_soles ON ventas(fecha_orig, soles);",
    "CREATE INDEX IF NOT EXISTS idx_retorno_cliente_sku ON ventas(id_cliente, id_articulo, fecha_orig);",
    "CREATE INDEX IF NOT EXISTS idx_retorno_folio ON ventas(folio_unico);",
    "CREATE INDEX IF NOT EXISTS idx_retorno_ref ON ventas(factura_ref_serie, factura_ref_nro);",
];

/// Tabla de cache para dashboard: evita full-scan en BD grande (791 MB / 1.1M filas).
/// Se refresca tras cada insercion/borrado via refresh_stats_cache().
pub const CREATE_STATS_CACHE_SQL: &str = "
CREATE TABLE IF NOT EXISTS stats_cache (
    key TEXT PRIMARY KEY,
    value REAL NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_stats_key ON stats_cache(key);
";

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
    // NC totales (>=99% cantidad vendida) por factura referencia — para precio neto
    r#"
    CREATE VIEW IF NOT EXISTS vw_nc_totales AS
    WITH base AS (
      SELECT v.id as venta_id, v.serie_doc, v.nro_doc, v.cantidad as cant_venta
      FROM ventas v WHERE v.tpo_doc LIKE 'F01%'
    )
    SELECT b.serie_doc, b.nro_doc,
           SUM(abs(n.cantidad_fae)) as total_fae,
           SUM(abs(n.soles)) as total_monto,
           COUNT(*) as nc_count
    FROM base b
    JOIN ventas n ON n.factura_ref_serie = b.serie_doc AND n.factura_ref_nro = b.nro_doc
      AND n.tipo_operacion = 'ajuste_valor'
      AND abs(n.cantidad_fae) >= b.cant_venta * 0.99
    GROUP BY b.serie_doc, b.nro_doc
    "#,
    // NC parciales (<99%) — solo informativo, no afecta precio
    r#"
    CREATE VIEW IF NOT EXISTS vw_nc_parciales AS
    SELECT n.factura_ref_serie, n.factura_ref_nro, n.folio_unico as nc_folio,
           abs(n.cantidad_fae) as cantidad_fae, abs(n.soles) as monto
    FROM ventas n
    WHERE n.tipo_operacion = 'ajuste_valor'
      AND NOT EXISTS (
        SELECT 1 FROM vw_nc_totales t
        WHERE t.serie_doc = n.factura_ref_serie AND t.nro_doc = n.factura_ref_nro
      )
    "#,
    // ─── Cockpit de ventas (apps de campo) ──────────────────────────────────
    // Historial de venta por cliente+SKU con precios comparados (LAG).
    // La cadena LAG corre solo sobre 'venta' (ajuste_valor contaminaría el
    // precio); NC/ND se exponen via UNION ALL con precio NULL.
    r#"
    CREATE VIEW IF NOT EXISTS vw_historial_venta_cliente AS
    WITH cadena AS (
      SELECT id_cliente, nom_cliente, id_vendedor, nom_vendedor,
             id_articulo, nom_articulo, id_linea, nom_linea,
             folio_unico, tpo_doc, serie_doc, nro_doc,
             fecha_orig, mes_ref, tipo_operacion, cantidad, soles, precio_unitario,
             LAG(precio_unitario)    OVER w AS precio_anterior,
             LAG(fecha_orig)         OVER w AS fecha_anterior,
             LAG(precio_unitario, 2) OVER w AS precio_anterior2
      FROM ventas
      WHERE tipo_operacion = 'venta'
      WINDOW w AS (PARTITION BY id_cliente, id_articulo ORDER BY fecha_orig, id)
    )
    SELECT * FROM cadena
    UNION ALL
    SELECT id_cliente, nom_cliente, id_vendedor, nom_vendedor,
           id_articulo, nom_articulo, id_linea, nom_linea,
           folio_unico, tpo_doc, serie_doc, nro_doc,
           fecha_orig, mes_ref, tipo_operacion, cantidad, soles,
           NULL, NULL, NULL, NULL
    FROM ventas
    WHERE tipo_operacion IN ('ajuste_valor', 'devolucion')
    "#,
    // Radar de recompra: cadencia por cliente+SKU (fallback a línea si <3 compras).
    // 'VENCIDO' = dias_silencio > cadencia_efectiva * 1.5
    r#"
    CREATE VIEW IF NOT EXISTS vw_radar_recompra AS
    WITH compras AS (
      SELECT id_cliente, nom_cliente, id_articulo, nom_articulo,
             id_linea, nom_linea, fecha_orig, cantidad, precio_unitario,
             LAG(fecha_orig) OVER (PARTITION BY id_cliente, id_articulo
                                   ORDER BY fecha_orig) AS fecha_previa
      FROM ventas
      WHERE tipo_operacion = 'venta' AND cantidad > 0
    ),
    gaps AS (
      SELECT id_cliente, nom_cliente, id_articulo, nom_articulo,
             id_linea, nom_linea, fecha_orig, cantidad, precio_unitario,
             CAST(julianday(fecha_orig) - julianday(fecha_previa) AS INTEGER) AS dias_gap
      FROM compras
      WHERE fecha_previa IS NOT NULL
    ),
    cadencia_sku AS (
      SELECT id_cliente, nom_cliente, id_articulo, nom_articulo, id_linea, nom_linea,
             COUNT(*)                                AS n_compras,
             MAX(fecha_orig)                         AS ultima_compra,
             CAST(ROUND(AVG(dias_gap)) AS INTEGER)   AS dias_cadencia,
             CAST(ROUND(AVG(precio_unitario), 4) AS REAL) AS precio_promedio,
             CAST(SUM(cantidad) AS REAL) / MAX(CAST(julianday(MAX(fecha_orig)) - julianday(MIN(fecha_orig)) AS INTEGER), 1)
                                                     AS und_por_dia
      FROM gaps
      GROUP BY 1,2,3,4,5,6
    ),
    cadencia_linea AS (
      SELECT id_cliente, id_linea, CAST(ROUND(AVG(dias_gap)) AS INTEGER) AS cadencia_linea
      FROM gaps
      GROUP BY 1,2
    )
    SELECT cs.*,
           CAST(julianday('now') - julianday(cs.ultima_compra) AS INTEGER) AS dias_silencio,
           COALESCE(cs.dias_cadencia, cl.cadencia_linea)                   AS cadencia_efectiva,
           CASE
             WHEN CAST(julianday('now') - julianday(cs.ultima_compra) AS INTEGER)
                  > COALESCE(cs.dias_cadencia, cl.cadencia_linea) * 1.5
             THEN 'VENCIDO'
             ELSE 'OK'
           END AS estado_oportunidad
    FROM cadencia_sku cs
    LEFT JOIN cadencia_linea cl
      ON cl.id_cliente = cs.id_cliente AND cl.id_linea = cs.id_linea
     AND cs.n_compras < 3
    "#,
    // Facturas disponibles con saldo y precio neto — para App devoluciones (LIFO)
    r#"
    CREATE VIEW IF NOT EXISTS vw_facturas_disponibles AS
    WITH ventas_agg AS (
      SELECT 
        v.id, v.folio_unico, v.serie_doc, v.nro_doc,
        v.id_cliente, v.id_articulo, v.nom_articulo,
        v.fecha_orig, v.cantidad as cantidad_vendida,
        v.precio_unitario, v.moneda, v.mes_ref,
        COALESCE(SUM(CASE WHEN d.tipo_operacion='devolucion' THEN abs(d.cantidad) ELSE 0 END), 0) as devuelto
      FROM ventas v
      LEFT JOIN ventas d ON d.factura_ref_serie = v.serie_doc AND d.factura_ref_nro = v.nro_doc
        AND d.tipo_operacion = 'devolucion'
      WHERE v.tpo_doc LIKE 'F01%'
      GROUP BY v.id, v.folio_unico, v.serie_doc, v.nro_doc, v.id_cliente, v.id_articulo,
               v.nom_articulo, v.fecha_orig, v.cantidad, v.precio_unitario, v.moneda, v.mes_ref
    )
    SELECT va.*,
      va.cantidad_vendida - va.devuelto as saldo_disponible,
      CASE 
        WHEN nt.total_fae IS NOT NULL THEN 
          ROUND(va.precio_unitario - (nt.total_monto / nt.total_fae), 4)
        ELSE va.precio_unitario
      END as precio_para_devolucion,
      CASE 
        WHEN date(va.fecha_orig) < date('now', '-3 years') THEN 'FUERA_PERIOD'
        ELSE 'DENTRO_PERIOD'
      END as estado_periodo
    FROM ventas_agg va
    LEFT JOIN vw_nc_totales nt ON nt.serie_doc = va.serie_doc AND nt.nro_doc = va.nro_doc
    ORDER BY va.fecha_orig DESC
    "#,
];
