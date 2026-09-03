-- Optimización del radar: MATERIALIZED VIEW en lugar de vista regular.
-- Motivo: la vista regular recalcula window functions sobre 618K filas por
-- request (timeout en clientes grandes). Materializada precalcula y responde
-- ~10-30 ms. Se refresca nocturnamente con pg_cron (ver paso 5).
--
-- IMPORTANTE: se agrupa por (id_cliente, id_articulo) normalizando nombres con
-- MAX() para GARANTIZAR el índice único requerido por REFRESH CONCURRENTLY
-- (un mismo SKU pudo cambiar de nombre en el histórico).

-- 1. Eliminar la vista regular
DROP VIEW IF EXISTS public.vw_radar_recompra;

-- 2. Crear materializada
CREATE MATERIALIZED VIEW public.vw_radar_recompra AS
with compras as (
  select
    id_cliente, nom_cliente,
    id_articulo, nom_articulo, id_linea, nom_linea,
    fecha_orig, cantidad, precio_unitario,
    lag(fecha_orig) over (partition by id_cliente, id_articulo
                          order by fecha_orig) as fecha_previa
  from public.ventas
  where tipo_operacion = 'venta' and cantidad > 0
),
gaps as (
  select
    id_cliente, nom_cliente,
    id_articulo, nom_articulo, id_linea, nom_linea,
    fecha_orig, cantidad, precio_unitario,
    (fecha_orig - fecha_previa) as dias_gap
  from compras
  where fecha_previa is not null
),
cadencia_sku as (
  select
    id_cliente,
    max(nom_cliente)               as nom_cliente,
    id_articulo,
    max(nom_articulo)              as nom_articulo,
    max(id_linea)                  as id_linea,
    max(nom_linea)                 as nom_linea,
    count(*)                       as n_compras,
    max(fecha_orig)                as ultima_compra,
    round(avg(dias_gap))                                   as dias_cadencia,
    round((avg(precio_unitario))::numeric, 4)              as precio_promedio,
    sum(cantidad) / greatest(max(fecha_orig) - min(fecha_orig), 1)
                                   as und_por_dia
  from gaps
  group by 1, 3
),
cadencia_linea as (
  select id_cliente, id_linea, round(avg(dias_gap)) as cadencia_linea
  from gaps
  group by 1,2
)
select
  cs.*,
  (current_date - cs.ultima_compra)              as dias_silencio,
  coalesce(cs.dias_cadencia, cl.cadencia_linea)  as cadencia_efectiva,
  case
    when (current_date - cs.ultima_compra) >
         coalesce(cs.dias_cadencia, cl.cadencia_linea) * 1.5
    then 'VENCIDO'
    else 'OK'
  end                                            as estado_oportunidad
from cadencia_sku cs
left join cadencia_linea cl
  on cl.id_cliente = cs.id_cliente and cl.id_linea = cs.id_linea
 and cs.n_compras < 3;

-- 3. Índice único (REQUERIDO para REFRESH MATERIALIZED VIEW CONCURRENTLY)
CREATE UNIQUE INDEX IF NOT EXISTS idx_mv_radar_cliente_sku
  ON public.vw_radar_recompra (id_cliente, id_articulo);

-- 4. Grants (las MATERIALIZED VIEWS no reciben los grants automáticos de Supabase)
GRANT SELECT ON public.vw_radar_recompra TO anon, authenticated, service_role;

-- 5. (Opcional, recomendado) Refresh nocturno con pg_cron — tras el sync de 15:00 Peru
--    Ejecutar UNA sola vez (requiere extensión pg_cron habilitada en Database > Extensions):
-- select cron.schedule('refresh-radar', '0 21 * * *',
--   $$REFRESH MATERIALIZED VIEW CONCURRENTLY public.vw_radar_recompra$$);
