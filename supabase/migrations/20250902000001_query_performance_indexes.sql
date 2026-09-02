-- Optimización de consultas para apps downstream (CRM, reportes por vendedor/línea)
-- Contexto: tras carga masiva (618K filas) el planner queda sin estadísticas ->
-- full scans en TODO. ANALYZE restaura planes; los índices nuevos cubren los
-- patrones de consulta: por documento, folio, vendedor y línea de producto.

-- 1. Estadísticas del planner (obligatorio tras bulk load)
ANALYZE public.ventas;

-- 2. Vendedor: compuesto (vendedor, mes) — el reporte ERP compara vendedor x mes.
--    Prefijo izquierdo cubre también filtro por vendedor solo.
create index if not exists idx_venta_vendedor_mes on public.ventas (id_vendedor, mes_ref);
create index if not exists idx_venta_vendedor_nom on public.ventas (nom_vendedor);
drop index if exists idx_venta_vendedor_id;  -- por si se ejecutó la versión anterior de esta migración

-- 3. Línea de producto (se había dropeado por espacio; consultas por línea la requieren)
create index if not exists idx_venta_linea on public.ventas (id_linea);

-- 4. Búsqueda por número de documento suelto (sin serie/tipo)
create index if not exists idx_venta_nro_doc on public.ventas (nro_doc);

-- 5. Cliente+fecha reemplaza a cliente solo (mismo costo, cubre "facturas del cliente en período")
drop index if exists idx_venta_cliente;
create index if not exists idx_venta_cliente_fecha on public.ventas (id_cliente, fecha_orig);

-- Presupuesto: ~13 índices totales (~465 MB de 500 MB free tier)
-- Índices conservados: idx_venta_mes, idx_venta_cliente_fecha, idx_venta_doc_cliente,
--   idx_venta_sku, idx_venta_doc, idx_venta_fact_ref, idx_venta_fecha,
--   idx_retorno_cliente_sku, idx_retorno_folio, idx_venta_vendedor_mes,
--   idx_venta_vendedor_nom, idx_venta_linea, idx_venta_nro_doc
