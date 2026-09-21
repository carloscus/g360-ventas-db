-- vw_historial_articulo: historial mensual por SKU con evolucion de NOMBRE,
-- LINEA y PRECIO UNITARIO (comparado contra el mes anterior).
--
-- Para la app PWA / analistas que necesitan responder:
--   * ¿cada cuanto cambia el precio de un SKU?  -> p_u_venta + variacion_precio_pct
--   * ¿el SKU se renombro?                       -> cambio_nombre
--   * ¿el SKU cambio de linea?                   -> cambio_linea
--   * ¿como evoluciono el volumen/precio neto?  -> vendido/devuelto/p_u_neto
--
-- Clave estable = id_articulo (el SKU). El nombre/linea de cada mes es el del
-- ULTIMO movimiento de ese mes. La historia completa vive en la tabla de hechos
-- `ventas`; esta vista la resume por mes sin perder los cambios.
--
-- Patron de consulta (PostgREST):
--   GET /rest/v1/vw_historial_articulo?id_articulo=eq.011019&order=mes_ref
--
-- Convencion de signos: `devolucion` puede tener cantidad negativa (BD local) o
-- positiva. Se usa ABS() para exponer `devuelto` como magnitud y
-- `cantidad_neta = vendido - devuelto` es correcto en AMBOS casos.
-- `ajuste_valor` (descuento) no es movimiento fisico: solo aporta a soles_netos.

create or replace view public.vw_historial_articulo as
with por_mes as (
  select
    id_articulo,
    mes_ref,
    sum(case when tipo_operacion = 'venta'      then cantidad else 0 end) as vendido,
    sum(case when tipo_operacion = 'devolucion' then abs(cantidad) else 0 end) as devuelto,
    sum(case when tipo_operacion = 'venta'      then cantidad else 0 end)
      - sum(case when tipo_operacion = 'devolucion' then abs(cantidad) else 0 end) as cantidad_neta,
    round((sum(case when tipo_operacion = 'venta' then soles else 0 end))::numeric, 2) as soles_vendidos,
    round((sum(soles))::numeric, 2) as soles_netos,
    round((
      sum(case when tipo_operacion = 'venta' then soles else 0 end)
      / nullif(sum(case when tipo_operacion = 'venta' then cantidad else 0 end), 0)
    )::numeric, 4) as p_u_venta,
    round((
      sum(soles)
      / nullif(
          sum(case when tipo_operacion = 'venta' then cantidad else 0 end)
          - sum(case when tipo_operacion = 'devolucion' then abs(cantidad) else 0 end),
          0)
    )::numeric, 4) as p_u_neto,
    count(*)                              as n_lineas,
    count(distinct id_cliente)            as n_clientes
  from public.ventas
  where id_articulo != ''
  group by id_articulo, mes_ref
),
attr_mes as (
  select id_articulo, mes_ref, original_sku, nom_articulo, id_linea, nom_linea
  from (
    select id_articulo, mes_ref, original_sku, nom_articulo, id_linea, nom_linea,
           row_number() over (
             partition by id_articulo, mes_ref
             order by fecha_orig desc, id desc
           ) as rn
    from public.ventas
    where id_articulo != ''
  ) t
  where rn = 1
)
select
  a.id_articulo, a.original_sku, a.nom_articulo, a.id_linea, a.nom_linea,
  m.mes_ref,
  m.vendido, m.devuelto, m.cantidad_neta,
  m.soles_vendidos, m.soles_netos, m.p_u_venta, m.p_u_neto,
  m.n_lineas, m.n_clientes,
  lag(m.p_u_venta)    over w as p_u_venta_anterior,
  lag(m.p_u_neto)     over w as p_u_neto_anterior,
  lag(a.nom_articulo) over w as nom_articulo_anterior,
  lag(a.id_linea)     over w as id_linea_anterior,
  lag(m.mes_ref)      over w as mes_anterior,
  case when a.nom_articulo is distinct from lag(a.nom_articulo) over w
       then 1 else 0 end as cambio_nombre,
  case when a.id_linea is distinct from lag(a.id_linea) over w
       then 1 else 0 end as cambio_linea,
  case when lag(m.p_u_venta) over w is not null and lag(m.p_u_venta) over w <> 0
       then round((
              (m.p_u_venta - lag(m.p_u_venta) over w) * 100.0
              / lag(m.p_u_venta) over w
            )::numeric, 2)
       else null end as variacion_precio_pct
from por_mes m
join attr_mes a
  on a.id_articulo = m.id_articulo and a.mes_ref = m.mes_ref
window w as (partition by a.id_articulo order by m.mes_ref);

comment on view public.vw_historial_articulo is
  'Historial mensual por SKU: evolucion de nombre, linea y precio unitario (con variacion vs mes anterior). Clave estable id_articulo.';

-- Seguridad: misma politica que el resto de vistas (solo lectura, security_invoker)
alter view public.vw_historial_articulo set (security_invoker = true);
grant select on public.vw_historial_articulo to anon, authenticated;

-- Verificacion
select v.viewname, c.reloptions
from pg_views v
join pg_class c on c.relname = v.viewname
where v.schemaname = 'public'
  and v.viewname = 'vw_historial_articulo';
