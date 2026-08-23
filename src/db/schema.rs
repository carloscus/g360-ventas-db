pub const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS ventas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    id_articulo TEXT NOT NULL, original_sku TEXT, nom_articulo TEXT,
    id_linea TEXT NOT NULL, nom_linea TEXT,
    id_grupo TEXT, nom_grupo TEXT,
    id_tipo TEXT, nom_tipo TEXT,
    id_familia TEXT, nom_familia TEXT,
    estado_linea TEXT,
    id_cliente TEXT NOT NULL, doc_cliente TEXT, nom_cliente TEXT,
    tpo_doc TEXT NOT NULL, serie_doc TEXT, nro_doc TEXT,
    referencia TEXT, moneda TEXT DEFAULT 'Soles',
    cantidad REAL NOT NULL, soles REAL NOT NULL, dolares REAL, precio_unitario REAL,
    anho INTEGER NOT NULL, mes INTEGER NOT NULL,
    fecha_orig TEXT NOT NULL,
    fecha_ref TEXT, fecha_venc TEXT, fec_cargo TEXT,
    cod_sucursal TEXT, nom_sucursal TEXT,
    departamento TEXT, provincia TEXT, distrito TEXT,
    canal_dist TEXT,
    id_vendedor TEXT, nom_vendedor TEXT,
    id_pedido TEXT,
    file_source TEXT, mes_ref TEXT NOT NULL,
    capturado_en TEXT DEFAULT (datetime('now'))
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
];
