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
    id_pedido TEXT, ord_compra TEXT,
    file_source TEXT, mes_ref TEXT NOT NULL,
    capturado_en TEXT DEFAULT (datetime('now')),
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

-- Estado de sincronizacion por mes con Supabase (reconciliacion resumible).
-- Reemplaza el full-window purge: solo los meses con checksum distinto se
-- purgan + re-suben, y el estado persiste entre corridas (checkpoint).
CREATE TABLE IF NOT EXISTS sync_months (
    mes_ref TEXT PRIMARY KEY,
    local_checksum TEXT NOT NULL,
    remote_checksum TEXT,
    estado TEXT NOT NULL DEFAULT 'pending', -- pending | synced | error
    filas INTEGER DEFAULT 0,
    intentos INTEGER DEFAULT 0,
    ultimo_error TEXT,
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
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

-- Dimensión RUC: agrupa clientes por RUC para evitar duplicados
-- Un RUC puede tener múltiples id_cliente históricos (renombres/fusiones ERP)
-- Se usa el nom_cliente más reciente como versión canonical
CREATE TABLE IF NOT EXISTS dim_ruc (
    ruc TEXT PRIMARY KEY,
    nom_cliente TEXT NOT NULL,
    departamento TEXT,
    provincia TEXT,
    distrito TEXT,
    n_clientes_unicos INTEGER DEFAULT 0,
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_dim_ruc_nombre ON dim_ruc(nom_cliente);
";

pub const CREATE_INDEXES_SQL: &[&str] = &[
    // ── Limpieza de índices REDUNDANTES (idempotente, una vez) ──────────
    // Cada índice extra encarece cada INSERT del capture y agranda la BD.
    // - idx_venta_linea_exact  == idx_venta_linea (duplicado exacto)
    // - idx_retorno_ref        == idx_venta_fact_ref (duplicado exacto)
    // - idx_venta_doc_cliente  = prefijo de idx_venta_doc_cliente_id
    // - idx_venta_fecha        = prefijo de idx_venta_fecha_soles
    // - idx_venta_linea_sufijo = substr(id_linea,-2), compat de formato viejo (ya normalizado)
    "DROP INDEX IF EXISTS idx_venta_linea_exact;",
    "DROP INDEX IF EXISTS idx_retorno_ref;",
    "DROP INDEX IF EXISTS idx_venta_doc_cliente;",
    "DROP INDEX IF EXISTS idx_venta_fecha;",
    "DROP INDEX IF EXISTS idx_venta_linea_sufijo;",
    // ── Índices vigentes ────────────────────────────────────────────────
    "CREATE INDEX IF NOT EXISTS idx_venta_mes ON ventas(mes_ref);",
    "CREATE INDEX IF NOT EXISTS idx_venta_cliente ON ventas(id_cliente);",
    "CREATE INDEX IF NOT EXISTS idx_venta_sku ON ventas(id_articulo);",
    "CREATE INDEX IF NOT EXISTS idx_venta_orig_sku ON ventas(original_sku);",
    "CREATE INDEX IF NOT EXISTS idx_venta_linea ON ventas(id_linea);",
    // Index para dedup y lookups por folio+articulo
    "CREATE INDEX IF NOT EXISTS idx_venta_folio_articulo ON ventas(folio_unico, id_articulo);",
    // Index para verificacion de RUC duplicados (doc_cliente + id_cliente)
    "CREATE INDEX IF NOT EXISTS idx_venta_doc_cliente_id ON ventas(doc_cliente, id_cliente);",
    "CREATE INDEX IF NOT EXISTS idx_venta_doc ON ventas(tpo_doc, serie_doc, nro_doc);",
    // Busqueda directa por (serie_doc, nro_doc): sin esto el EXISTS del filtro
    // NC/ND (with_line_filter) cae en idx_venta_linea y el plan se vuelve
    // O(n^2) sobre toda la tabla -> stats/upload tardaban horas. Fix 2026-09-15.
    "CREATE INDEX IF NOT EXISTS idx_venta_serie_nro ON ventas(serie_doc, nro_doc);",
    "CREATE INDEX IF NOT EXISTS idx_venta_ref ON ventas(referencia);",
    "CREATE INDEX IF NOT EXISTS idx_venta_tipo_op ON ventas(tipo_operacion);",
    "CREATE INDEX IF NOT EXISTS idx_venta_fact_ref ON ventas(factura_ref_serie, factura_ref_nro);",
    "CREATE INDEX IF NOT EXISTS idx_venta_soles ON ventas(soles);",
    "CREATE INDEX IF NOT EXISTS idx_venta_fecha_soles ON ventas(fecha_orig, soles);",
    "CREATE INDEX IF NOT EXISTS idx_retorno_cliente_sku ON ventas(id_cliente, id_articulo, fecha_orig);",
    "CREATE INDEX IF NOT EXISTS idx_retorno_folio ON ventas(folio_unico);",
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

-- Agregado mensual precomputado para el dashboard (evita GROUP BY en vivo sobre 2.8M).
-- Mismo filtro de lineas que el uploader -> coherente con Supabase.
CREATE TABLE IF NOT EXISTS stats_por_mes (
    mes_ref TEXT PRIMARY KEY,
    filas INTEGER NOT NULL DEFAULT 0,
    ventas_soles REAL NOT NULL DEFAULT 0,
    clientes INTEGER NOT NULL DEFAULT 0,
    cantidad REAL NOT NULL DEFAULT 0,
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
);
";

/// Versión del esquema/semántica de las tablas `dim_*`. Se guarda en
/// `PRAGMA user_version` para forzar UN rebuild en bases existentes cuando
/// cambia la semántica (no solo columnas), evitando reconstruir en cada arranque.
///   v1 -> inicial
///   v2 -> 2026-09-15: atributos por "fila más reciente" en vez de MAX()
///         alfabético (nombres vigentes al renombrar cliente/SKU/línea/vendedor).
///   v3 -> 2026-09-15: agrega fact_venta_mes (comparativos YoY/MoM instantáneos).
pub const DIM_SCHEMA_VERSION: i64 = 3;

/// Tablas de dimensiones derivadas de ventas.
/// Cada tabla se reconstruye con refresh_dim_tables() tras cada dedup/insert.
/// Son la fuente de verdad para navegación local (clientes, vendedores, documentos, SKUs, lineas).
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
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
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
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
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
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
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
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
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
    actualizado_en TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_dim_linea_grupo ON dim_linea(id_grupo);

-- Agregado mensual precomputado por linea+SKU (fact table derivada).
-- Existe para que los comparativos por periodo (YoY/MoM) sean INSTANTANEOS:
-- una vista equivalente agregaria los 2.8M de filas en cada consulta (~60s),
-- mientras que esta tabla (decenas de miles de filas) responde en milisegundos.
-- Se reconstruye en refresh_dim_tables junto con las dims.
CREATE TABLE IF NOT EXISTS fact_venta_mes (
    id_linea TEXT NOT NULL,
    id_articulo TEXT NOT NULL,
    mes_ref TEXT NOT NULL,
    cantidad REAL DEFAULT 0,      -- unidades vendidas (tipo_operacion='venta')
    soles REAL DEFAULT 0,         -- monto vendido
    devuelto REAL DEFAULT 0,      -- unidades devueltas (magnitud positiva)
    soles_netos REAL DEFAULT 0,   -- monto neto (ventas - devoluciones/ajustes)
    PRIMARY KEY (id_linea, id_articulo, mes_ref)
);
CREATE INDEX IF NOT EXISTS idx_fvm_mes ON fact_venta_mes(mes_ref);
CREATE INDEX IF NOT EXISTS idx_fvm_linea ON fact_venta_mes(id_linea, mes_ref);
CREATE INDEX IF NOT EXISTS idx_fvm_sku ON fact_venta_mes(id_articulo, mes_ref);
";

/// Vistas listas para consumir por CRM / dashboard / plataforma de devoluciones.
/// Son VIEWS virtuales (no ocupan almacenamiento extra; se calculan al consultar).
pub const CREATE_VIEWS_SQL: &[&str] = &[
    // ─── Dimensiones ─────────────────────────────────────────────────
    // Clientes unicos (fuente de verdad: cada cliente con su ultimo nombre conocido).
    // DROP + CREATE (no IF NOT EXISTS) para que la definicion del codigo siempre
    // reemplace una vista existente: las vistas son metadatos (recrear es instantaneo).
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
    // Articulos unicos: todas las dimensiones de producto (nombre vigente)
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
    // Lineas / categorias del ERP (jerarquia de producto).
    // Clave estable = id_linea; nombres por fila MAS RECIENTE (si el ERP renombra
    // una linea sin cambiar el codigo, se muestra el nombre vigente).
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
    // Historial por SKU x mes: evolucion de NOMBRE, LINEA y PRECIO UNITARIO.
    // Clave estable = id_articulo; el nombre/linea de cada mes es el del ULTIMO
    // movimiento de ese mes. LAG da el mes anterior, los flags cambio_* marcan
    // renombres/reclasificaciones y variacion_precio_pct el salto de precio.
    // Patron de consulta: WHERE id_articulo = ? ORDER BY mes_ref
    //
    // Convencion de signos: `devolucion` puede venir con cantidad negativa (local)
    // o positiva (Supabase tiene check cantidad>=0). Se usa ABS() para expresar
    // `devuelto` como magnitud positiva y `cantidad_neta = vendido - devuelto` es
    // correcto en AMBOS casos. `ajuste_valor` (descuento) no es movimiento fisico:
    // solo aporta a soles_netos, no a las cantidades.
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
    // Comparativo por SKU x linea x mes: cantidad y monto del periodo vs
    // año anterior (YoY) y mes anterior (MoM), con variacion %. Lee de la tabla
    // precomputada `fact_venta_mes` para responder en ms (una vista sobre ventas
    // agregaria 2.8M filas por consulta). El mismo-mes-otro-año se resuelve por
    // join exacto al mes_ref calculado (LAG no sirve si faltan meses).
    // Patron: WHERE id_linea=? AND mes_ref=?   |   WHERE id_articulo=?
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
    // Comparativo por LINEA x mes: agrega vw_comparativo_ventas y recalcula el %
    // al nivel de linea. Solo suma al comparativo los SKUs que EXISTEN en el mes
    // de referencia (cantidad_comparable) para que el % sea peras-con-peras y no
    // se infle por SKUs nuevos.
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
