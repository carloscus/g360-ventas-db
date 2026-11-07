/// Schema de la base de datos canonica para g360-ventas-db.
/// Diseñado para soportar historial desde 2010 hasta hoy,
/// con versionado de contrato, esquema y normalización.
///
/// Conceptos clave:
/// - CONTRACTO_VERSION: formato de tabla `ventas` y tablas auditables.
///   Si un consumidor (cliente offline) lee una versión distinta, bloquea.
/// - DIM_SCHEMA_VERSION: semántica de las tablas `dim_*`. Usa `PRAGMA user_version`.
/// - NORMALIZACION_VERSION: reglas de saneamiento aplicadas. No bloquea consumidores.
///
/// Nota: El código actual vive en schema.rs para centralizarlo y evitar drifts
/// entre versiones documentadas y reales.

// ─────────────────────────────────────────────────────────────────────────────
// Versiones de contrato
// ─────────────────────────────────────────────────────────────────────────────

/// Versión del esquema/semántica de las tablas `dim_*`. Se guarda en
/// `PRAGMA user_version` para forzar UN rebuild en bases existentes cuando
/// cambia la semántica (no solo columnas).
///   v1 -> inicial
///   v2 -> 2026-09-15: atributos por "fila más reciente"
///   v3 -> 2026-09-15: agrega fact_venta_mes (comparativos YoY/MoM instantáneos).
pub const DIM_SCHEMA_VERSION: i64 = 3;

/// Versión del contrato de datos entre productor (g360-ventas-db) y consumidores.
/// Se almacena en `g360_meta.contracto_version`.
/// Los consumidores leen esta tabla (NO PRAGMA user_version) para decidir si bloquean.
/// Bump cuando: nueva columna en `ventas`, nueva tabla del contrato, cambio
/// de semántica de columna existente. No bump: índices, vistas, tablas locales.
pub const CONTRACTO_VERSION: i64 = 4;

/// Versión de las reglas de saneamiento/normalización aplicadas a los datos.
/// Se almacena en `g360_meta.normalizacion_version`.
/// Bump cuando cambia una regla de normalización; el consumidor NO bloquea.
/// v2 (06/11): pad de `id_vendedor` a 3 chars (anomalía de fuente '0154' ->
/// '054') + fix del off-by-one de SUBSTR en la reparación legacy. El bump
/// hace que el gate de init_pool() ejecute las reparaciones una sola vez más.
pub const NORMALIZACION_VERSION: i64 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Tabla principal: ventas (normalizada, con CHECKs para historial extensivo)
// ─────────────────────────────────────────────────────────────────────────────

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
    id_vendedor TEXT NOT NULL, nom_vendedor TEXT,
    id_pedido TEXT, ord_compra TEXT,
    file_source TEXT, mes_ref TEXT NOT NULL,
    capturado_en TEXT DEFAULT '',
    tipo_operacion TEXT DEFAULT 'venta',
    factura_ref_serie TEXT,
    factura_ref_nro TEXT,
    folio_unico TEXT,
    id_ubigeo TEXT,
    canal_distribucion TEXT,
    nom_condicion_pago TEXT,
    estado_linea TEXT,
    division TEXT,
    fec_cargo TEXT,
    id_guia TEXT
)";

// ─────────────────────────────────────────────────────────────────────────────
// Tablas de auditoría e integridad
// ─────────────────────────────────────────────────────────────────────────────

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
    started_at TEXT NOT NULL DEFAULT '',
    finished_at TEXT
);

-- Metadata del contrato: versiones y estado.
-- Consumidores leen desde aquí (NO de PRAGMA user_version).
-- key='contracto_version' -> valor = CONTRACTO_VERSION (leido por consumidores)
-- key='dim_schema_version' -> valor = DIM_SCHEMA_VERSION (leido por init_pool)
-- key='normalizacion_version' -> valor = NORMALIZACION_VERSION
CREATE TABLE IF NOT EXISTS g360_meta (
    key TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL,
    actualizado_en TEXT NOT NULL DEFAULT ''
);

-- Checksums mensuales para detectar cambios
CREATE TABLE IF NOT EXISTS mes_checksums (
    mes_ref TEXT NOT NULL,
    checksum TEXT NOT NULL,
    total_filas INTEGER NOT NULL,
    total_soles REAL NOT NULL,
    total_cantidad REAL,
    calculado_en TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (mes_ref)
);

-- Checksums diarios para ETL incremental por día (publish_incremental).
-- Key: COUNT(*) || ':' || ROUND(COALESCE(SUM(soles), 0), 2).
-- publish_incremental compara este valor con el último publicado en etl_state.
-- NOTA: calculado_en se llena desde calculate_day_checksums() con timestamp
-- pasado como parametro, porque DEFAULT '' no es determinista
-- en SQLite in-memory y causa error de no-determinismo en el indice PRIMARY KEY.
CREATE TABLE IF NOT EXISTS day_checksums (
    dia TEXT NOT NULL PRIMARY KEY,
    filas INTEGER NOT NULL,
    soles REAL NOT NULL,
    checksum TEXT NOT NULL,
    calculado_en TEXT
);

-- Estado de sincronizacion por mes con Supabase (reconciliacion resumible).
CREATE TABLE IF NOT EXISTS sync_months (
    mes_ref TEXT PRIMARY KEY,
    local_checksum TEXT NOT NULL,
    remote_checksum TEXT,
    estado TEXT NOT NULL DEFAULT 'pending', -- pending | synced | error
    filas INTEGER DEFAULT 0,
    intentos INTEGER DEFAULT 0,
    ultimo_error TEXT,
    actualizado_en TEXT NOT NULL DEFAULT ''
);

-- Estado del ETL incremental hacia Supabase (por día).
-- Keys: 'ultimo_dia_publicado' = '2026-11-03'
CREATE TABLE IF NOT EXISTS etl_state (
    key TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT ''
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
    creado_en TEXT NOT NULL DEFAULT ''
);
";

// ─────────────────────────────────────────────────────────────────────────────
// Índices (optimizados para queries frecuentes)
// ─────────────────────────────────────────────────────────────────────────────

pub const CREATE_INDEXES_SQL: &[&str] = &[
    // Index para busquedas directas por mes_ref (rango 5 años)
    "CREATE INDEX IF NOT EXISTS idx_venta_mes ON ventas(mes_ref);",
    // Index para busquedas directas por cliente
    "CREATE INDEX IF NOT EXISTS idx_venta_cliente ON ventas(id_cliente);",
    // Index para busquedas directas por SKU
    "CREATE INDEX IF NOT EXISTS idx_venta_sku ON ventas(id_articulo);",
    // Index para busquedas directas por original_sku
    "CREATE INDEX IF NOT EXISTS idx_venta_orig_sku ON ventas(original_sku);",
    // Index para busquedas directas por línea
    "CREATE INDEX IF NOT EXISTS idx_venta_linea ON ventas(id_linea);",
    // Index para dedup y lookups por folio+articulo
    "CREATE INDEX IF NOT EXISTS idx_venta_folio_articulo ON ventas(folio_unico, id_articulo);",
    // Index para verificacion de RUC duplicados (doc_cliente + id_cliente)
    "CREATE INDEX IF NOT EXISTS idx_venta_doc_cliente_id ON ventas(doc_cliente, id_cliente);",
    // Index para verificacion por documento completo
    "CREATE INDEX IF NOT EXISTS idx_venta_doc ON ventas(tpo_doc, serie_doc, nro_doc);",
    // Index para busquedas directas por serie+nro (sin cruzar con línea)
    "CREATE INDEX IF NOT EXISTS idx_venta_serie_nro ON ventas(serie_doc, nro_doc);",
    // Index para busquedas directas por referencia
    "CREATE INDEX IF NOT EXISTS idx_venta_ref ON ventas(referencia);",
    // Index para busquedas directas por tipo_operacion
    "CREATE INDEX IF NOT EXISTS idx_venta_tipo_op ON ventas(tipo_operacion);",
    // Index para cruzar factura con NC/ND
    "CREATE INDEX IF NOT EXISTS idx_venta_fact_ref ON ventas(factura_ref_serie, factura_ref_nro);",
    // Index para consultas monetarias
    "CREATE INDEX IF NOT EXISTS idx_venta_soles ON ventas(soles);",
    // Index para ordenar por fecha y monto (queries de ultimos 5 anos)
    "CREATE INDEX IF NOT EXISTS idx_venta_fecha_soles ON ventas(fecha_orig, soles);",
    // Index para devolver por cliente+articulo+fecha
    "CREATE INDEX IF NOT EXISTS idx_retorno_cliente_sku ON ventas(id_cliente, id_articulo, fecha_orig);",
    // Index para dedup por folio unico
    "CREATE INDEX IF NOT EXISTS idx_retorno_folio ON ventas(folio_unico);",
    // Index para ordenar por fecha en historicos (queries 2010-2018)
    // NOTA: sin WHERE date('now') porque no es determinista en SQLite in-memory.
    // En producción se puede agregar via ALTER TABLE ADD INDEX si es necesario.
    "CREATE INDEX IF NOT EXISTS idx_venta_fecha_historico ON ventas(fecha_orig);",
    // Indices parciales para la sanacion de precio_unitario de init_pool.
    // IMPORTANTE (empirico, 06/11): NO unirlos con OR en el WHERE parcial -
    // SQLite no prueba la implicancia Wq => (X OR Y) cuando la rama Y es una
    // conjuncion y el UPDATE del caso fae caia en SCAN (31 s). Dos indices
    // separados, cada uno con la predicate EXACTA de su UPDATE -> SEARCH.
    // Cubren solo las filas a reparar (casi vacios -> inserts casi gratis).
    "DROP INDEX IF EXISTS idx_venta_precio_heal;",
    "CREATE INDEX IF NOT EXISTS idx_venta_precio_null ON ventas(precio_unitario) \
        WHERE precio_unitario IS NULL;",
    "CREATE INDEX IF NOT EXISTS idx_venta_precio_fae ON ventas(precio_unitario, cantidad, cantidad_fae) \
        WHERE precio_unitario = 0 AND cantidad = 0 AND cantidad_fae != 0;",
];

// ─────────────────────────────────────────────────────────────────────────────
// Tablas de cache para dashboard (performance)
// ─────────────────────────────────────────────────────────────────────────────

pub const CREATE_STATS_CACHE_SQL: &str = "
CREATE TABLE IF NOT EXISTS stats_cache (
    key TEXT PRIMARY KEY,
    value REAL NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_stats_key ON stats_cache(key);

-- Agregado mensual precomputado para el dashboard (evita GROUP BY en vivo sobre millones de filas).
CREATE TABLE IF NOT EXISTS stats_por_mes (
    mes_ref TEXT PRIMARY KEY,
    filas INTEGER NOT NULL DEFAULT 0,
    ventas_soles REAL NOT NULL DEFAULT 0,
    clientes INTEGER NOT NULL DEFAULT 0,
    cantidad REAL NOT NULL DEFAULT 0,
    actualizado_en TEXT NOT NULL DEFAULT ''
);
";

// ─────────────────────────────────────────────────────────────────────────────
// Tablas de dimensiones derivadas de ventas
// ─────────────────────────────────────────────────────────────────────────────

pub const CREATE_DIM_TABLES_SQL: &str = "
-- Clientes unicos (ultima info conocida + KPIs)
CREATE TABLE IF NOT EXISTS dim_cliente (
    id_cliente TEXT PRIMARY KEY,
    nom_cliente TEXT NOT NULL,
    doc_cliente TEXT,
    departamento TEXT,
    provincia TEXT,
    distrito TEXT,
    n_ventas INTEGER DEFAULT 0,
    total_soles REAL DEFAULT 0,
    total_dolares REAL DEFAULT 0,
    ultima_compra TEXT,
    actualizado_en TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_dim_cliente_nombre ON dim_cliente(nom_cliente);
CREATE INDEX IF NOT EXISTS idx_dim_cliente_ruc ON dim_cliente(doc_cliente);

-- Vendedores unicos
CREATE TABLE IF NOT EXISTS dim_vendedor (
    id_vendedor TEXT PRIMARY KEY,
    nom_vendedor TEXT NOT NULL,
    n_ventas INTEGER DEFAULT 0,
    total_soles REAL DEFAULT 0,
    n_clientes INTEGER DEFAULT 0,
    ultima_venta TEXT,
    actualizado_en TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_dim_vendedor_nombre ON dim_vendedor(nom_vendedor);

-- Articulos/SKUs unicos (ultima info de catalogo)
CREATE TABLE IF NOT EXISTS dim_articulo (
    id_articulo TEXT PRIMARY KEY,
    original_sku TEXT,
    nom_articulo TEXT NOT NULL,
    id_linea TEXT,
    nom_linea TEXT,
    id_grupo TEXT,
    nom_grupo TEXT,
    id_tipo TEXT,
    nom_tipo TEXT,
    id_familia TEXT,
    nom_familia TEXT,
    n_ventas INTEGER DEFAULT 0,
    total_cantidad REAL DEFAULT 0,
    total_soles REAL DEFAULT 0,
    precio_promedio REAL DEFAULT 0,
    ultima_venta TEXT,
    actualizado_en TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_dim_articulo_sku ON dim_articulo(original_sku);
CREATE INDEX IF NOT EXISTS idx_dim_articulo_nombre ON dim_articulo(nom_articulo);
CREATE INDEX IF NOT EXISTS idx_dim_articulo_linea ON dim_articulo(id_linea);

-- Cabeceras de documentos (una fila por folio unico)
CREATE TABLE IF NOT EXISTS dim_documento (
    folio_unico TEXT PRIMARY KEY,
    tpo_doc TEXT NOT NULL,
    serie_doc TEXT NOT NULL,
    nro_doc TEXT NOT NULL,
    fecha_orig TEXT NOT NULL,
    id_cliente TEXT NOT NULL,
    id_vendedor TEXT,
    n_lineas INTEGER DEFAULT 0,
    total_cantidad REAL DEFAULT 0,
    total_soles REAL DEFAULT 0,
    total_dolares REAL DEFAULT 0,
    moneda TEXT DEFAULT 'Soles',
    actualizado_en TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_dim_documento_cliente ON dim_documento(id_cliente, fecha_orig);
CREATE INDEX IF NOT EXISTS idx_dim_documento_vendedor ON dim_documento(id_vendedor, fecha_orig);
CREATE INDEX IF NOT EXISTS idx_dim_documento_fecha ON dim_documento(fecha_orig);
CREATE INDEX IF NOT EXISTS idx_dim_documento_serie_nro ON dim_documento(serie_doc, nro_doc);

-- Lineas/categorias unicas
CREATE TABLE IF NOT EXISTS dim_linea (
    id_linea TEXT PRIMARY KEY,
    nom_linea TEXT,
    id_grupo TEXT,
    nom_grupo TEXT,
    id_familia TEXT,
    nom_familia TEXT,
    n_ventas INTEGER DEFAULT 0,
    total_soles REAL DEFAULT 0,
    n_clientes INTEGER DEFAULT 0,
    actualizado_en TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_dim_linea_grupo ON dim_linea(id_grupo);

-- Agregado mensual precomputado por linea+SKU (fact table derivada).
-- Existe para que los comparativos por periodo (YoY/MoM) sean INSTANTANEOS.
CREATE TABLE IF NOT EXISTS fact_venta_mes (
    id_linea TEXT NOT NULL,
    id_articulo TEXT NOT NULL,
    mes_ref TEXT NOT NULL,
    cantidad REAL DEFAULT 0,
    soles REAL DEFAULT 0,
    devuelto REAL DEFAULT 0,
    soles_netos REAL DEFAULT 0,
    PRIMARY KEY (id_linea, id_articulo, mes_ref)
);
CREATE INDEX IF NOT EXISTS idx_fvm_mes ON fact_venta_mes(mes_ref);
CREATE INDEX IF NOT EXISTS idx_fvm_linea ON fact_venta_mes(id_linea, mes_ref);
CREATE INDEX IF NOT EXISTS idx_fvm_sku ON fact_venta_mes(id_articulo, mes_ref);
";

// ─────────────────────────────────────────────────────────────────────────────
// Vistas listas para consumir por CRM / dashboard
// ─────────────────────────────────────────────────────────────────────────────

pub const CREATE_VIEWS_SQL: &[&str] = &[
    // ─── Dimensiones ─────────────────────────────────────────────────
    r#"
    DROP VIEW IF EXISTS vw_dim_cliente;
    CREATE VIEW vw_dim_cliente AS
    SELECT id_cliente, doc_cliente, nom_cliente, departamento, provincia, distrito
    FROM (
      SELECT id_cliente, doc_cliente, nom_cliente, departamento, provincia, distrito,
             ROW_NUMBER() OVER (PARTITION BY id_cliente ORDER BY fecha_orig DESC, id DESC) rn
      FROM ventas WHERE id_cliente != ''
    ) WHERE rn = 1
    "#,
    // Articulos unicos
    r#"
    DROP VIEW IF EXISTS vw_dim_articulo;
    CREATE VIEW vw_dim_articulo AS
    SELECT id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
           id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia
    FROM (
      SELECT id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
             id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia,
             ROW_NUMBER() OVER (PARTITION BY id_articulo ORDER BY fecha_orig DESC, id DESC) rn
      FROM ventas WHERE id_articulo != ''
    ) WHERE rn = 1
    "#,
    // Lineas / categorias del ERP
    r#"
    DROP VIEW IF EXISTS vw_dim_linea;
    CREATE VIEW vw_dim_linea AS
    SELECT id_linea, nom_linea, id_grupo, nom_grupo, id_familia, nom_familia
    FROM (
      SELECT id_linea, nom_linea, id_grupo, nom_grupo, id_familia, nom_familia,
             ROW_NUMBER() OVER (
                 PARTITION BY id_linea
                 ORDER BY CASE WHEN nom_linea != '' THEN 0 ELSE 1 END,
                          fecha_orig DESC, id DESC
             ) rn
      FROM ventas WHERE id_linea != ''
    ) WHERE rn = 1
    "#,
    // ─── Hechos / analisis ───────────────────────────────────────────
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
    // ─── Comparativos YoY/MoM ───────────────────────────────────────────
    r#"
    DROP VIEW IF EXISTS vw_comparativo_ventas;
    CREATE VIEW vw_comparativo_ventas AS
    SELECT
      p.id_linea, p.id_articulo, p.mes_ref,
      p.cantidad, p.soles, p.devuelto, p.soles_netos,
      y.cantidad AS cantidad_anio_ant, y.soles AS soles_anio_ant,
      CASE WHEN y.cantidad IS NOT NULL AND y.cantidad != 0
           THEN ROUND((p.cantidad - y.cantidad) * 100.0 / y.cantidad, 2) END AS var_cantidad_pct,
      CASE WHEN y.soles IS NOT NULL AND y.soles != 0
           THEN ROUND((p.soles - y.soles) * 100.0 / y.soles, 2) END AS var_soles_pct,
      m.cantidad AS cantidad_mes_ant, m.soles AS soles_mes_ant,
      CASE WHEN m.cantidad IS NOT NULL AND m.cantidad != 0
           THEN ROUND((p.cantidad - m.cantidad) * 100.0 / m.cantidad, 2) END AS var_cantidad_mom_pct,
      CASE WHEN m.soles IS NOT NULL AND m.soles != 0
           THEN ROUND((p.soles - m.soles) * 100.0 / m.soles, 2) END AS var_soles_mom_pct
    FROM fact_venta_mes p
    LEFT JOIN fact_venta_mes y
      ON y.id_linea = p.id_linea AND y.id_articulo = p.id_articulo
     AND y.mes_ref = strftime('%Y-%m', date(p.mes_ref || '-01', '-12 months'))
    LEFT JOIN fact_venta_mes m
      ON m.id_linea = p.id_linea AND m.id_articulo = p.id_articulo
     AND m.mes_ref = strftime('%Y-%m', date(p.mes_ref || '-01', '-1 month'))
    "#,
    r#"
    DROP VIEW IF EXISTS vw_comparativo_linea;
    CREATE VIEW vw_comparativo_linea AS
    SELECT
      id_linea, mes_ref,
      SUM(cantidad) AS cantidad, SUM(soles) AS soles,
      SUM(devuelto) AS devuelto, SUM(soles_netos) AS soles_netos,
      SUM(cantidad_anio_ant) AS cantidad_anio_ant,
      SUM(soles_anio_ant)    AS soles_anio_ant,
      SUM(CASE WHEN cantidad_anio_ant IS NOT NULL THEN cantidad ELSE 0 END) AS cantidad_comparable,
      SUM(CASE WHEN soles_anio_ant    IS NOT NULL THEN soles    ELSE 0 END) AS soles_comparable,
      CASE WHEN SUM(soles_anio_ant) != 0
           THEN ROUND((SUM(CASE WHEN soles_anio_ant IS NOT NULL THEN soles ELSE 0 END)
                       - SUM(soles_anio_ant)) * 100.0 / SUM(soles_anio_ant), 2) END AS var_soles_pct,
      CASE WHEN SUM(cantidad_anio_ant) != 0
           THEN ROUND((SUM(CASE WHEN cantidad_anio_ant IS NOT NULL THEN cantidad ELSE 0 END)
                       - SUM(cantidad_anio_ant)) * 100.0 / SUM(cantidad_anio_ant), 2) END AS var_cantidad_pct,
      SUM(cantidad_mes_ant) AS cantidad_mes_ant,
      SUM(soles_mes_ant)    AS soles_mes_ant
    FROM vw_comparativo_ventas
    GROUP BY id_linea, mes_ref
    "#,
    // ─── Historial por SKU x mes (evolución de precio/nombre/línea) ────
    r#"
    CREATE VIEW IF NOT EXISTS vw_historial_articulo AS
    WITH por_mes AS (
      SELECT
        id_articulo, mes_ref,
        SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END) AS vendido,
        SUM(CASE WHEN tipo_operacion = 'devolucion' THEN ABS(cantidad) ELSE 0 END) AS devuelto,
        SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END)
          - SUM(CASE WHEN tipo_operacion = 'devolucion' THEN ABS(cantidad) ELSE 0 END) AS cantidad_neta,
        ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END), 2) AS soles_vendidos,
        ROUND(SUM(soles), 2) AS soles_netos,
        ROUND(SUM(CASE WHEN tipo_operacion = 'venta' THEN soles ELSE 0 END)
              / NULLIF(SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END), 0), 4) AS p_u_venta,
        ROUND(SUM(soles)
              / NULLIF(SUM(CASE WHEN tipo_operacion = 'venta' THEN cantidad ELSE 0 END)
                       - SUM(CASE WHEN tipo_operacion = 'devolucion' THEN ABS(cantidad) ELSE 0 END), 0), 4) AS p_u_neto,
        COUNT(*) AS n_lineas,
        COUNT(DISTINCT id_cliente) AS n_clientes
      FROM ventas
      WHERE id_articulo != ''
      GROUP BY id_articulo, mes_ref
    ),
    attr_mes AS (
      SELECT id_articulo, mes_ref, original_sku, nom_articulo, id_linea, nom_linea
      FROM (
        SELECT id_articulo, mes_ref, original_sku, nom_articulo, id_linea, nom_linea,
               ROW_NUMBER() OVER (
                 PARTITION BY id_articulo, mes_ref
                 ORDER BY fecha_orig DESC, id DESC
               ) AS rn
        FROM ventas WHERE id_articulo != ''
      ) WHERE rn = 1
    )
    SELECT
      a.id_articulo, a.original_sku, a.nom_articulo, a.id_linea, a.nom_linea,
      m.mes_ref,
      m.vendido, m.devuelto, m.cantidad_neta,
      m.soles_vendidos, m.soles_netos, m.p_u_venta, m.p_u_neto,
      m.n_lineas, m.n_clientes,
      LAG(m.p_u_venta)    OVER w AS p_u_venta_anterior,
      LAG(m.p_u_neto)     OVER w AS p_u_neto_anterior,
      LAG(a.nom_articulo) OVER w AS nom_articulo_anterior,
      LAG(a.id_linea)     OVER w AS id_linea_anterior,
      LAG(m.mes_ref)      OVER w AS mes_anterior,
      CASE WHEN a.nom_articulo != LAG(a.nom_articulo) OVER w THEN 1 ELSE 0 END AS cambio_nombre,
      CASE WHEN a.id_linea != LAG(a.id_linea) OVER w THEN 1 ELSE 0 END AS cambio_linea,
      CASE WHEN LAG(m.p_u_venta) OVER w IS NOT NULL AND LAG(m.p_u_venta) OVER w != 0
           THEN ROUND((m.p_u_venta - LAG(m.p_u_venta) OVER w) * 100.0
                      / LAG(m.p_u_venta) OVER w, 2)
           ELSE NULL END AS variacion_precio_pct
    FROM por_mes m
    JOIN attr_mes a ON a.id_articulo = m.id_articulo AND a.mes_ref = m.mes_ref
    WINDOW w AS (PARTITION BY a.id_articulo ORDER BY m.mes_ref)
    "#,
    // ─── Cockpit de ventas (apps de campo) ──────────────────────────────────
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
];
