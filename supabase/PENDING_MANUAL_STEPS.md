# PASOS MANUALES PENDIENTES EN SUPABASE

> Estos pasos NO se pueden automatizar desde la app (PostgREST no ejecuta DDL).
> Ejecutar en: https://supabase.com/dashboard/project/tqdmoytaucnfrpaklprc/sql
> IMPORTANTE: desactivar el toggle "Limit" (read-only) en el SQL Editor.

Estado: ~~PENDIENTE~~ **EJECUTADAS** (2026-09-02). Verificado por API:
- `vw_historial_venta_cliente` ✅ (200 con anon key; cadena precios OK, TAI LOY/02211 = 3.3248 estable)
- `vw_radar_recompra` ✅ creada — **pero timeout si no se filtra por `id_cliente` acotado**
  (calcula window functions sobre 618K filas por request). El app DEBE filtrar
  por cliente (o lista pequeña `id_cliente=in.(...)`). Si se requiere radar
  global sin filtro: convertir a MATERIALIZED VIEW + refresh nocturno.
- Índices ✅ aplicados; latencias ficha cliente+periodo: ~400 ms (PostgREST
  baseline ~300 ms incluido). Radar por cliente: 3.4 s (aceptable, mejorable
  con materialized view si el usuario lo pide).
- Fix aplicado durante ejecución: `round(double precision, int)` no existe en
  PG → cast `::numeric`; `cantidad` faltante en CTEs compras/gaps.

---

## BLOQUE 1 — Índices + estadísticas (migración 20250902000001)

```sql
ANALYZE public.ventas;

create index if not exists idx_venta_vendedor_cliente on public.ventas (id_vendedor, id_cliente);
drop index if exists idx_venta_vendedor_mes;
drop index if exists idx_venta_vendedor_nom;
drop index if exists idx_venta_vendedor_id;

create index if not exists idx_venta_linea on public.ventas (id_linea);
create index if not exists idx_venta_nro_doc on public.ventas (nro_doc);

drop index if exists idx_venta_cliente;
create index if not exists idx_venta_cliente_fecha on public.ventas (id_cliente, fecha_orig);
```

## BLOQUE 2 — Vistas del cockpit (migración 20250902000002)

```sql
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
    round(avg(dias_gap))                                   as dias_cadencia,
    round((avg(precio_unitario))::numeric, 4)              as precio_promedio,
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
```

## BLOQUE 3 — Verificación (pegar resultados)

```sql
-- Índices (deben ser 12)
SELECT indexrelname FROM pg_stat_user_indexes WHERE relname='ventas';

-- Vista historial (TAI LOY + 02211): debe mostrar la cadena de precios
SELECT fecha_orig, cantidad, precio_unitario, precio_anterior
FROM vw_historial_venta_cliente
WHERE id_cliente='00002482' AND id_articulo='02211' AND tipo_operacion='venta'
ORDER BY fecha_orig DESC LIMIT 5;

-- Vista radar: conteo de oportunidades
SELECT estado_oportunidad, COUNT(*) FROM vw_radar_recompra GROUP BY 1;
```

## Nota sobre el radar (Fase 3 del cockpit — refinamiento pendiente)

El radar con ventana de 4 años incluye SKUs abandonados hace años
(ej: silencio=3162 días). Antes de exponerlo al app, la Fase 3 debe:
1. Filtrar **clientes activos** (última compra de cualquier SKU < ~180 días)
2. Agrupar compras del **mismo día** como una sola (la cadencia actual cuenta
   cada factura individual: un cliente con 10 facturas el mismo día da cadencia ~0)
3. Ventana máxima de silencio a evaluar (ej. < 730 días)
