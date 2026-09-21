-- Comparativos por SKU y por LINEA: CANTIDAD y MONTO del periodo vs
-- AÑO ANTERIOR (YoY) y MES ANTERIOR (MoM), con variacion %.
--
-- Performance: una vista normal tendria que agregar ~2.8M filas de `ventas` en
-- CADA consulta (~decenas de segundos, igual que la version local). Por eso el
-- agregado mensual se precomputa en una MATERIALIZED VIEW (mv_venta_linea_sku_mes)
-- y los comparativos son vistas ligeras encima -> respuesta en ms.
-- Se refresca con pg_cron (ver al final), igual que vw_radar_recompra.
--
-- Uso (PostgREST):
--   * Linea, un mes:  /rest/v1/vw_comparativo_linea?id_linea=eq.01&mes_ref=eq.2026-08
--   * Linea, serie:   /rest/v1/vw_comparativo_linea?id_linea=eq.01&order=mes_ref
--   * SKU:            /rest/v1/vw_comparativo_ventas?id_articulo=eq.011019&order=mes_ref
--   * SKUs de linea:  /rest/v1/vw_comparativo_ventas?id_linea=eq.01&mes_ref=eq.2026-08

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. MATERIALIZED VIEW: agregado mensual por linea + SKU
--    devuelto = magnitud positiva (robusto a la convencion de signos)
--    soles_netos = ventas - NC/ND/ajustes
-- ─────────────────────────────────────────────────────────────────────────────
drop materialized view if exists public.mv_venta_linea_sku_mes;
create materialized view public.mv_venta_linea_sku_mes as
select
  id_linea,
  id_articulo,
  mes_ref,
  sum(case when tipo_operacion = 'venta' then cantidad else 0 end)                as cantidad,
  round((sum(case when tipo_operacion = 'venta' then soles else 0 end))::numeric, 2) as soles,
  sum(case when tipo_operacion = 'devolucion' then abs(cantidad) else 0 end)      as devuelto,
  round((sum(soles))::numeric, 2)                                                 as soles_netos
from public.ventas
where id_articulo != '' and id_linea != ''
group by id_linea, id_articulo, mes_ref;

-- Indice unico (REQUERIDO para REFRESH ... CONCURRENTLY)
create unique index if not exists idx_mv_vlsm_linea_sku_mes
  on public.mv_venta_linea_sku_mes (id_linea, id_articulo, mes_ref);
-- Indices de acceso directo
create index if not exists idx_mv_vlsm_mes   on public.mv_venta_linea_sku_mes (mes_ref);
create index if not exists idx_mv_vlsm_sku   on public.mv_venta_linea_sku_mes (id_articulo, mes_ref);

comment on materialized view public.mv_venta_linea_sku_mes is
  'Agregado mensual por linea+SKU (cantidad/monto/devuelto/neto). Base de los comparativos YoY/MoM. Refrescar con pg_cron.';

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Comparativo por SKU+linea+mes (YoY + MoM)
-- ─────────────────────────────────────────────────────────────────────────────
create or replace view public.vw_comparativo_ventas as
select
  p.id_linea, p.id_articulo, p.mes_ref,
  p.cantidad, p.soles, p.devuelto, p.soles_netos,
  y.cantidad as cantidad_anio_ant,
  y.soles    as soles_anio_ant,
  case when y.cantidad is not null and y.cantidad <> 0
       then round(((p.cantidad - y.cantidad) * 100.0 / y.cantidad)::numeric, 2) end as var_cantidad_pct,
  case when y.soles is not null and y.soles <> 0
       then round(((p.soles - y.soles) * 100.0 / y.soles)::numeric, 2) end as var_soles_pct,
  m.cantidad as cantidad_mes_ant,
  m.soles    as soles_mes_ant,
  case when m.cantidad is not null and m.cantidad <> 0
       then round(((p.cantidad - m.cantidad) * 100.0 / m.cantidad)::numeric, 2) end as var_cantidad_mom_pct,
  case when m.soles is not null and m.soles <> 0
       then round(((p.soles - m.soles) * 100.0 / m.soles)::numeric, 2) end as var_soles_mom_pct
from public.mv_venta_linea_sku_mes p
left join public.mv_venta_linea_sku_mes y
  on y.id_linea = p.id_linea and y.id_articulo = p.id_articulo
 and y.mes_ref = to_char((p.mes_ref || '-01')::date - interval '12 months', 'YYYY-MM')
left join public.mv_venta_linea_sku_mes m
  on m.id_linea = p.id_linea and m.id_articulo = p.id_articulo
 and m.mes_ref = to_char((p.mes_ref || '-01')::date - interval '1 month', 'YYYY-MM');

comment on view public.vw_comparativo_ventas is
  'Comparativo por SKU+linea+mes: cantidad/monto vs año anterior (YoY) y mes anterior (MoM), con variacion %.';

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Comparativo por LINEA+mes (rollup). `*_comparable` suma solo los SKUs con
--    año anterior para que el % sea peras-con-peras (un SKU nuevo no infla).
-- ─────────────────────────────────────────────────────────────────────────────
create or replace view public.vw_comparativo_linea as
select
  id_linea, mes_ref,
  sum(cantidad)    as cantidad,
  sum(soles)       as soles,
  sum(devuelto)    as devuelto,
  sum(soles_netos) as soles_netos,
  sum(cantidad_anio_ant) as cantidad_anio_ant,
  sum(soles_anio_ant)    as soles_anio_ant,
  sum(case when cantidad_anio_ant is not null then cantidad else 0 end) as cantidad_comparable,
  sum(case when soles_anio_ant    is not null then soles    else 0 end) as soles_comparable,
  case when sum(soles_anio_ant) <> 0
       then round(((sum(case when soles_anio_ant is not null then soles else 0 end)
                    - sum(soles_anio_ant)) * 100.0 / sum(soles_anio_ant))::numeric, 2) end as var_soles_pct,
  case when sum(cantidad_anio_ant) <> 0
       then round(((sum(case when cantidad_anio_ant is not null then cantidad else 0 end)
                    - sum(cantidad_anio_ant)) * 100.0 / sum(cantidad_anio_ant))::numeric, 2) end as var_cantidad_pct,
  sum(cantidad_mes_ant) as cantidad_mes_ant,
  sum(soles_mes_ant)    as soles_mes_ant
from public.vw_comparativo_ventas
group by id_linea, mes_ref;

comment on view public.vw_comparativo_linea is
  'Comparativo por linea+mes (rollup de vw_comparativo_ventas). var_*_pct usa solo SKUs con año anterior.';

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Seguridad (las vistas heredan RLS via security_invoker; la MV lleva grants)
-- ─────────────────────────────────────────────────────────────────────────────
alter view public.vw_comparativo_ventas set (security_invoker = true);
alter view public.vw_comparativo_linea  set (security_invoker = true);
grant select on public.vw_comparativo_ventas to anon, authenticated;
grant select on public.vw_comparativo_linea  to anon, authenticated;
grant select on public.mv_venta_linea_sku_mes to anon, authenticated, service_role;

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. Refresh (ejecutar UNA vez; requiere pg_cron en Database > Extensions).
--    Tras el sync diario de Tauri. Ajustar la hora segun el cron real.
-- ─────────────────────────────────────────────────────────────────────────────
-- select cron.schedule('refresh-comparativo',
--   '0 22 * * *',
--   $$refresh materialized view concurrently public.mv_venta_linea_sku_mes$$);

-- ─────────────────────────────────────────────────────────────────────────────
-- 6. Verificacion
-- ─────────────────────────────────────────────────────────────────────────────
select v.viewname, c.reloptions
from pg_views v
join pg_class c on c.relname = v.viewname
where v.schemaname = 'public'
  and v.viewname in ('vw_comparativo_ventas', 'vw_comparativo_linea')
order by v.viewname;
