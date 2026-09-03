-- Optimización de consultas para apps downstream (cockpit de ventas, CRM, devoluciones)
-- Contexto: tras carga masiva (618K filas) el planner queda sin estadísticas ->
-- full scans en TODO. ANALYZE restaura planes; los índices nuevos cubren los
-- patrones de consulta reales:
--   * Navegación "clientes del vendedor"      -> (id_vendedor, id_cliente)
--   * Ficha cliente + histórico por período   -> (id_cliente, fecha_orig)
--   * Búsqueda por línea de producto          -> (id_linea)
--   * Búsqueda por nro. de documento suelto   -> (nro_doc)
-- El vendedor NO filtra cálculos de devolución (es jerarquía/navegación),
-- por eso no se indexa (vendedor, mes) ni nom_vendedor.

-- 1. Estadísticas del planner (obligatorio tras bulk load)
ANALYZE public.ventas;

-- 2. Vendedor -> cliente (navegación jerárquica; index-only scan para DISTINCT id_cliente)
create index if not exists idx_venta_vendedor_cliente on public.ventas (id_vendedor, id_cliente);
drop index if exists idx_venta_vendedor_mes;   -- por si se ejecutó la versión anterior de esta migración
drop index if exists idx_venta_vendedor_nom;
drop index if exists idx_venta_vendedor_id;

-- 3. Línea de producto (se había dropeado por espacio; consultas por línea la requieren)
create index if not exists idx_venta_linea on public.ventas (id_linea);

-- 4. Búsqueda por número de documento suelto (sin serie/tipo)
create index if not exists idx_venta_nro_doc on public.ventas (nro_doc);

-- 5. Cliente+fecha reemplaza a cliente solo (mismo costo, cubre "facturas del cliente en período")
drop index if exists idx_venta_cliente;
create index if not exists idx_venta_cliente_fecha on public.ventas (id_cliente, fecha_orig);

-- Presupuesto: 12 índices totales (~440 MB de 500 MB free tier)
-- Índices finales: idx_venta_mes, idx_venta_cliente_fecha, idx_venta_doc_cliente,
--   idx_venta_sku, idx_venta_doc, idx_venta_fact_ref, idx_venta_fecha,
--   idx_retorno_cliente_sku, idx_retorno_folio, idx_venta_vendedor_cliente,
--   idx_venta_linea, idx_venta_nro_doc
