// Tablas para auditoría e integridad de datos

pub const CREATE_AUDIT_TABLES_SQL: &str = "
-- Log de sincronizaciones: registra cada sync con conteos
CREATE TABLE IF NOT EXISTS sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tipo TEXT NOT NULL DEFAULT 'upload',  -- 'upload', 'reparse', 'import', 'manual'
    estado TEXT NOT NULL DEFAULT 'pending',  -- 'pending', 'in_progress', 'completed', 'failed'
    filas_solicitadas INTEGER DEFAULT 0,
    filas_subidas INTEGER DEFAULT 0,
    filas_limpiadas INTEGER DEFAULT 0,
    duracion_segundos REAL,
    error_message TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT
);

-- Checksums mensuales: hash para detectar cambios en datos
CREATE TABLE IF NOT EXISTS mes_checksums (
    mes_ref TEXT NOT NULL,
    checksum TEXT NOT NULL,
    total_filas INTEGER NOT NULL,
    total_soles REAL NOT NULL,
    calculado_en TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (mes_ref)
);

-- Auditoría de cambios: registro de cada operación DML
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tabla TEXT NOT NULL,
    operacion TEXT NOT NULL,  -- 'INSERT', 'UPDATE', 'DELETE'
    folio_unico TEXT,
    id_articulo TEXT,
    filas_afectadas INTEGER DEFAULT 1,
    detalle TEXT,
    creado_en TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Indices para queries eficientes
CREATE INDEX IF NOT EXISTS idx_sync_log_fecha ON sync_log(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_sync_log_tipo ON sync_log(tipo);
CREATE INDEX IF NOT EXISTS idx_audit_log_folio ON audit_log(folio_unico);
CREATE INDEX IF NOT EXISTS idx_audit_log_fecha ON audit_log(creado_en DESC);
";

// SQL para generar checksum de un mes específico
pub const CALCULATE_MONTH_CHECKSUM_SQL: &str = "
INSERT INTO mes_checksums (mes_ref, checksum, total_filas, total_soles, calculado_en)
SELECT 
    mes_ref,
    -- Generar hash simple basado en conteos y totales
    printf('%08x-%08x-%08x', 
        COUNT(*),
        CAST(ROUND(SUM(soles) * 100) AS INTEGER) & 0xFFFFFFFF,
        CAST(ROUND(SUM(dolares * 100), 0) AS INTEGER) & 0xFFFFFFFF
    ) as checksum,
    COUNT(*) as total_filas,
    ROUND(SUM(soles), 2) as total_soles,
    datetime('now')
FROM ventas
WHERE mes_ref = ?
GROUP BY mes_ref
ON CONFLICT(mes_ref) DO UPDATE SET
    checksum = excluded.checksum,
    total_filas = excluded.total_filas,
    total_soles = excluded.total_soles,
    calculado_en = excluded.calculado_en;
";

// SQL para verificar integridad de un folio específico
pub const VERIFY_FOLIO_SQL: &str = "
WITH csv_folios AS (
    -- Aquí se insertarían los folios del CSV
    SELECT ? as folio_unico, ? as nro_doc, ? as serie_doc, ? as tpo_doc
),
bd_folios AS (
    SELECT folio_unico, COUNT(*) as lineas, SUM(cantidad) as total_cant, SUM(soles) as total_soles
    FROM ventas
    WHERE folio_unico = ?
    GROUP BY folio_unico
)
SELECT 
    COALESCE(cf.folio_unico, 'N/A') as folio,
    COALESCE(bf.lineas, 0) as lineas_bd,
    COALESCE(bf.total_cant, 0) as cantidad_bd,
    COALESCE(bf.total_soles, 0) as soles_bd
FROM csv_folios cf
LEFT JOIN bd_folios bf ON cf.folio_unico = bf.folio_unico;
";
