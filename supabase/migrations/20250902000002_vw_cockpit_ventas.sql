-- Vistas para g360-ventas-cockpit (app de campo para vendedores)
-- Consumo via PostgREST: /rest/v1/vw_historial_venta_cliente, /rest/v1/vw_radar_recompra

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. HISTORIAL VENTA CLIENTE (líneas de venta + precios comparados vía LAG)
--    Patrón de consulta del app:
--      GET /rest/v1/vw_historial_venta_cliente
--          ?id_cliente=eq.X&id_vendedor=eq.Y
--          &fecha_orig=gte.2025-01-01&fecha_orig=lte.2025-12-31
--          &order=id_articulo.asc,fecha_orig.desc
--    El app agrupa por id_articulo -> una fila por SKU:
--      precio_unitario (último), precio_anterior, precio_anterior2
--
--    IMPORTANTE: la cadena LAG corre SOLO sobre filas 'venta' (un ajuste_valor
--    tiene precio negativo y contaminaría precio_anterior). Las filas NC/ND se
--    exponen en la misma vista via UNION ALL (precio NULL) para que el app
--    agregue descuentos/devoluciones por SKU con una sola consulta.
-- ─────────────────────────────────────────────────────────────────────────────
create or replace view public.vw_historial_venta_cliente as
with cadena as (
  select
    id_cliente, nom_cliente,
    id_vendedor, nom_vendedor,
    id_articulo, nom_articulo, id_linea, nom_linea,
    folio_unico, tpo_doc, serie_doc, nro_doc,
    fecha_orig, mes_ref, tipo_operacion, cantidad, soles, precio_unitario,
    lag(precio_unitario)    over w as precio_anterior,
    lag(fecha_orig)         over w as fecha_anterior,
    lag(precio_unitario, 2) over w as precio_anterior2
  from public.ventas
  where tipo_operacion = 'venta'
  window w as (partition by id_cliente, id_articulo order by fecha_orig, id)
)
select * from cadena
union all
select
  id_cliente, nom_cliente, id_vendedor, nom_vendedor,
  id_articulo, nom_articulo, id_linea, nom_linea,
  folio_unico, tpo_doc, serie_doc, nro_doc,
  fecha_orig, mes_ref, tipo_operacion, cantidad, soles,
  null::double precision as precio_unitario,
  null::double precision as precio_anterior,
  null::date             as fecha_anterior,
  null::double precision as precio_anterior2
from public.ventas
where tipo_operacion in ('ajuste_valor', 'devolucion');

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. RADAR DE RECOMPRA (cadencia por cliente+SKU -> oportunidad de visita)
--    'VENCIDO' = dias_silencio > cadencia_efectiva * 1.5
--    Fallback a línea cuando el SKU tiene < 3 compras (cadencia poco confiable).
--    und_por_dia permite estimar la oportunidad (und_por_dia * dias de atraso).
--    Nivel cliente+SKU (sin partir por vendedor): el app filtra por los clientes
--    del vendedor (jerarquía), la cadencia pertenece al cliente.
-- ─────────────────────────────────────────────────────────────────────────────
create or replace view public.vw_radar_recompra as
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
    id_cliente, nom_cliente,
    id_articulo, nom_articulo, id_linea, nom_linea,
    count(*)                       as n_compras,
    max(fecha_orig)                as ultima_compra,
    round(avg(dias_gap))           as dias_cadencia,
    round(avg(precio_unitario), 4) as precio_promedio,
    sum(cantidad) / greatest(max(fecha_orig) - min(fecha_orig), 1)
                                   as und_por_dia
  from gaps
  group by 1,2,3,4,5,6
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
