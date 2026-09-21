-- verify_deploy.sql
-- Ejecutar en BD nueva para validar deploy correcto

-- 1. Tablas críticas existen
SELECT 'ventas' as tabla, COUNT(*) as row_count FROM ventas
UNION ALL SELECT 'dim_ruc', COUNT(*) FROM dim_ruc
UNION ALL SELECT 'mes_checksums', COUNT(*) FROM mes_checksums
UNION ALL SELECT 'sync_log', COUNT(*) FROM sync_log;

-- 2. Índices críticos existen
SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='ventas' ORDER BY name;

-- 3. RUCs problemáticos (deben ser 0 después de limpieza)
SELECT doc_cliente, COUNT(*) as cnt FROM ventas 
WHERE doc_cliente NOT GLOB '[0-9]*' AND doc_cliente != ''
GROUP BY doc_cliente HAVING cnt > 10;

-- 4. Meses sin datos (agujeros)
WITH meses AS (
    SELECT DISTINCT mes_ref FROM ventas
),
rango AS (
    SELECT min(mes_ref) as inicio, max(mes_ref) as fin FROM meses
    -- Generar todos los meses del rango
)
SELECT * FROM meses 
WHERE mes_ref NOT IN (SELECT DISTINCT mes_ref FROM ventas);
