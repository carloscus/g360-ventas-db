// Script SQL para crear un cartucho ejemplo con datos sintéticos.
// Uso: sqlite3 cartucho_ejemplo.db < create_sample_cartucho.sql
// O ejecutar via Rust/Python para mayor control.

-- Tabla principal (mismo schema que el productor)
CREATE TABLE IF NOT EXISTS ventas (
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
    departamento TEXT, provincia TEXT, distrito TEXT,
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
);

-- Metadata del contrato
CREATE TABLE IF NOT EXISTS g360_meta (
    key TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL,
    actualizado_en TEXT DEFAULT ''
);

-- Checksums mensuales
CREATE TABLE IF NOT EXISTS mes_checksums (
    mes_ref TEXT NOT NULL PRIMARY KEY,
    checksum TEXT NOT NULL,
    total_filas INTEGER NOT NULL,
    total_soles REAL NOT NULL,
    total_cantidad REAL,
    calculado_en TEXT DEFAULT ''
);

-- Checksums diarios
CREATE TABLE IF NOT EXISTS day_checksums (
    dia TEXT NOT NULL PRIMARY KEY,
    filas INTEGER NOT NULL,
    soles REAL NOT NULL,
    checksum TEXT NOT NULL,
    calculado_en TEXT
);

-- Sembrar metadata de versiones
INSERT OR REPLACE INTO g360_meta (key, value, actualizado_en) VALUES
    ('contracto_version', '4', datetime('now')),
    ('dim_schema_version', '3', datetime('now')),
    ('normalizacion_version', '1', datetime('now'));

-- ─── DATOS DE EJEMPLO ───────────────────────────────────────────────
-- 3 meses de datos sintéticos: 2025-10, 2025-11, 2025-12

-- Ventas de octubre 2025
INSERT INTO ventas (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
    id_cliente, doc_cliente, nom_cliente, tpo_doc, serie_doc, nro_doc,
    cantidad, soles, dolares, precio_unitario, cantidad_fae,
    anho, mes, fecha_orig, mes_ref, tipo_operacion, folio_unico, id_vendedor, nom_vendedor) VALUES
('SKU001','SKU001','PRODUCTO A','01','LINEA A','00000001','20123456789','CLIENTE DEMO SAC','F01','204','10001',
 10, 150.00, 0, 15.00, 10, 2025, 10, '2025-10-05', '2025-10', 'venta', 'F01/204/10001','101','VENDEDOR 1'),
('SKU001','SKU001','PRODUCTO A','01','LINEA A','00000001','20123456789','CLIENTE DEMO SAC','F01','204','10002',
 5, 75.00, 0, 15.00, 5, 2025, 10, '2025-10-12', '2025-10', 'venta', 'F01/204/10002','101','VENDEDOR 1'),
('SKU002','SKU002','PRODUCTO B','02','LINEA B','00000002','20987654321','OTRO CLIENTE EIRL','F01','204','10003',
 20, 300.00, 40.00, 15.00, 20, 2025, 10, '2025-10-15', '2025-10', 'venta', 'F01/204/10003','102','VENDEDOR 2'),
('SKU003','SKU003','PRODUCTO C','78','LINEA C','00000001','20123456789','CLIENTE DEMO SAC','F01','204','10004',
 3, 450.00, 0, 150.00, 3, 2025, 10, '2025-10-20', '2025-10', 'venta', 'F01/204/10004','101','VENDEDOR 1'),
-- NC de octubre (devolución parcial)
('SKU001','SKU001','PRODUCTO A','01','LINEA A','00000001','20123456789','CLIENTE DEMO SAC','NCR','204','90001',
 -2, -30.00, 0, 15.00, -2, 2025, 10, '2025-10-25', '2025-10', 'devolucion', 'NCR/204/90001','101','VENDEDOR 1');

-- Ventas de noviembre 2025
INSERT INTO ventas (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
    id_cliente, doc_cliente, nom_cliente, tpo_doc, serie_doc, nro_doc,
    cantidad, soles, dolares, precio_unitario, cantidad_fae,
    anho, mes, fecha_orig, mes_ref, tipo_operacion, folio_unico, id_vendedor, nom_vendedor) VALUES
('SKU001','SKU001','PRODUCTO A','01','LINEA A','00000001','20123456789','CLIENTE DEMO SAC','F01','204','11001',
 8, 120.00, 0, 15.00, 8, 2025, 11, '2025-11-03', '2025-11', 'venta', 'F01/204/11001','101','VENDEDOR 1'),
('SKU002','SKU002','PRODUCTO B','02','LINEA B','00000002','20987654321','OTRO CLIENTE EIRL','F01','204','11002',
 15, 225.00, 30.00, 15.00, 15, 2025, 11, '2025-11-10', '2025-11', 'venta', 'F01/204/11002','102','VENDEDOR 2'),
('SKU004','SKU004','PRODUCTO D','MA','LINEA D','00000003','20555555555','TERCER CLIENTE SA','F01','205','11003',
 1, 1500.00, 200.00, 1500.00, 1, 2025, 11, '2025-11-18', '2025-11', 'venta', 'F01/205/11003','103','VENDEDOR 3'),
-- NC de noviembre (devolución total)
('SKU002','SKU002','PRODUCTO B','02','LINEA B','00000002','20987654321','OTRO CLIENTE EIRL','NCR','204','90002',
 -15, -225.00, -30.00, 15.00, -15, 2025, 11, '2025-11-28', '2025-11', 'devolucion', 'NCR/204/90002','102','VENDEDOR 2');

-- Ventas de diciembre 2025
INSERT INTO ventas (id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
    id_cliente, doc_cliente, nom_cliente, tpo_doc, serie_doc, nro_doc,
    cantidad, soles, dolares, precio_unitario, cantidad_fae,
    anho, mes, fecha_orig, mes_ref, tipo_operacion, folio_unico, id_vendedor, nom_vendedor) VALUES
('SKU001','SKU001','PRODUCTO A','01','LINEA A','00000001','20123456789','CLIENTE DEMO SAC','F01','204','12001',
 12, 180.00, 0, 15.00, 12, 2025, 12, '2025-12-02', '2025-12', 'venta', 'F01/204/12001','101','VENDEDOR 1'),
('SKU003','SKU003','PRODUCTO C','78','LINEA C','00000001','20123456789','CLIENTE DEMO SAC','F01','204','12002',
 4, 600.00, 80.00, 150.00, 4, 2025, 12, '2025-12-10', '2025-12', 'venta', 'F01/204/12002','101','VENDEDOR 1'),
('SKU004','SKU004','PRODUCTO D','MA','LINEA D','00000003','20555555555','TERCER CLIENTE SA','F01','205','12003',
 2, 3000.00, 400.00, 1500.00, 2, 2025, 12, '2025-12-15', '2025-12', 'venta', 'F01/205/12003','103','VENDEDOR 3'),
('SKU002','SKU002','PRODUCTO B','02','LINEA B','00000002','20987654321','OTRO CLIENTE EIRL','F01','204','12004',
 10, 150.00, 20.00, 15.00, 10, 2025, 12, '2025-12-20', '2025-12', 'venta', 'F01/204/12004','102','VENDEDOR 2');

-- Calcular checksums mensuales
INSERT OR REPLACE INTO mes_checksums (mes_ref, checksum, total_filas, total_soles, total_cantidad, calculado_en)
SELECT
    mes_ref,
    printf('%08x-%08x-%08x-%08x',
        COUNT(*),
        CAST(ROUND(SUM(soles) * 100) AS INTEGER) & 0xFFFFFFFF,
        CAST(ROUND(SUM(COALESCE(dolares, 0) * 100), 0) AS INTEGER) & 0xFFFFFFFF,
        CAST(ROUND(SUM(cantidad) * 100) AS INTEGER) & 0xFFFFFFFF
    ),
    COUNT(*),
    ROUND(SUM(soles), 2),
    ROUND(SUM(cantidad), 4),
    datetime('now')
FROM ventas
GROUP BY mes_ref;

-- Calcular checksums diarios
INSERT INTO day_checksums (dia, filas, soles, checksum, calculado_en)
SELECT
    fecha_orig,
    COUNT(*),
    ROUND(SUM(soles), 2),
    COUNT(*) || ':' || ROUND(SUM(soles), 2),
    datetime('now')
FROM ventas
WHERE fecha_orig IS NOT NULL AND fecha_orig != ''
GROUP BY fecha_orig;
