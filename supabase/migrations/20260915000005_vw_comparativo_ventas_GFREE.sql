-- Comparativos por SKU y LINEA para PLAN GRATUITO (sin materialized view)
-- Versión optimizada para espacios reducidos: solo vistas, 0 MB adicional
-- Performance: ~20s por consulta (vs instantánea en MV)

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Comparativo por SKU+linea+mes (YoY + MoM)
--    Sin materializar: lee directamente de ventas (consultas más lentas pero 0 espacio)
-- ─────────────────────────────────────────────────────────────────────────────
create or replace view public.vw_comparativo_ventas as
with por_mes as (
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
  group by id_linea, id_articulo, mes_ref
)
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
from por_mes p
left join por_mes y
  on y.id_linea = p.id_linea and y.id_articulo = p.id_articulo
 and y.mes_ref = to_char((p.mes_ref || '-01')::date - interval '12 months', 'YYYY-MM')
left join por_mes m
  on m.id_linea = p.id_linea and m.id_articulo = p.id_articulo
 and m.mes_ref = to_char((p.mes_ref || '-01')::date - interval '1 month', 'YYYY-MM');

comment on view public.vw_comparativo_ventas is
  'Comparativo por SKU+linea+mes (plan gratuito). YoY/MoM con variacion %.';

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Comparativo por LINEA+mes (rollup)
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
  'Comparativo por linea+mes (rollup de vw_comparativo_ventas, plan gratuito).';

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Seguridad
-- ─────────────────────────────────────────────────────────────────────────────
alter view public.vw_comparativo_ventas set (security_invoker = true);
alter view public.vw_comparativo_linea  set (security_invoker = true);
grant select on public.vw_comparativo_ventas to anon, authenticated;
grant select on public.vw_comparativo_linea  to anon, authenticated;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Verificacion
-- ─────────────────────────────────────────────────────────────────────────────
select v.viewname, c.reloptions
from pg_views v
join pg_class c on c.relname = v.viewname
where v.schemaname = 'public'
  and v.viewname in ('vw_comparativo_ventas', 'vw_comparativo_linea')
order by viewname;
