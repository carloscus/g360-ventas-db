-- Vistas para API de Devoluciones Fisicas
-- Uso: GET /rest/v1/retornos/{endpoint}

-- Dimension SKU (catalogo desde historico)
create or replace view public.vw_dim_articulo as
select id_articulo, original_sku, max(nom_articulo) as nom_articulo,
       max(id_linea) as id_linea, max(nom_linea) as nom_linea,
       max(id_grupo) as id_grupo, max(nom_grupo) as nom_grupo
from ventas where id_articulo != '' group by id_articulo, original_sku;

-- Dimension Cliente
create or replace view public.vw_dim_cliente as
select id_cliente, doc_cliente, max(nom_cliente) as nom_cliente
from ventas where id_cliente != '' group by id_cliente, doc_cliente;

-- NC totales (>=99% cantidad vendida) por factura referencia
create or replace view public.vw_nc_totales as
with base as (
  select v.id as venta_id, v.serie_doc, v.nro_doc, v.cantidad as cant_venta
  from ventas v where v.tpo_doc like 'F01%'
)
select b.serie_doc, b.nro_doc,
       sum(abs(n.cantidad_fae)) as total_fae,
       sum(abs(n.soles)) as total_monto,
       count(*) as nc_count
from base b
join ventas n on n.factura_ref_serie = b.serie_doc and n.factura_ref_nro = b.nro_doc
  and n.tipo_operacion = 'ajuste_valor'
  and abs(n.cantidad_fae) >= b.cant_venta * 0.99
group by b.serie_doc, b.nro_doc;

-- NC parciales (<99%) por factura referencia
create or replace view public.vw_nc_parciales as
select n.factura_ref_serie, n.factura_ref_nro, n.folio_unico as nc_folio,
       abs(n.cantidad_fae) as cantidad_fae, abs(n.soles) as monto
from ventas n
where n.tipo_operacion = 'ajuste_valor'
  and not exists (
    select 1 from vw_nc_totales t
    where t.serie_doc = n.factura_ref_serie and t.nro_doc = n.factura_ref_nro
  );

-- Facturas disponibles con saldo y precio neto
create or replace view public.vw_facturas_disponibles as
with ventas_agg as (
  select 
    v.id, v.folio_unico, v.serie_doc, v.nro_doc,
    v.id_cliente, v.id_articulo, v.nom_articulo,
    v.fecha_orig, v.cantidad as cantidad_vendida,
    v.precio_unitario, v.moneda, v.mes_ref,
    coalesce(sum(case when d.tipo_operacion='devolucion' then abs(d.cantidad) else 0 end), 0) as devuelto
  from ventas v
  left join ventas d on d.factura_ref_serie = v.serie_doc and d.factura_ref_nro = v.nro_doc
    and d.tipo_operacion = 'devolucion'
  where v.tpo_doc like 'F01%'
  group by v.id, v.folio_unico, v.serie_doc, v.nro_doc, v.id_cliente, v.id_articulo,
           v.nom_articulo, v.fecha_orig, v.cantidad, v.precio_unitario, v.moneda, v.mes_ref
),
precio_net as (
  select va.*,
    va.cantidad_vendida - va.devuelto as saldo_disponible,
    case 
      when nt.total_fae is not null then 
        round((va.precio_unitario - (nt.total_monto / nt.total_fae))::numeric, 4)
      else va.precio_unitario
    end as precio_para_devolucion,
    case 
      when va.fecha_orig < current_date - interval '3 years' then 'FUERA_PERIOD'
      else 'DENTRO_PERIOD'
    end as estado_periodo
  from ventas_agg va
  left join vw_nc_totales nt on nt.serie_doc = va.serie_doc and nt.nro_doc = va.nro_doc
)
select * from precio_net order by fecha_orig desc;

-- Indices
create index if not exists idx_retorno_cliente_sku on ventas(id_cliente, id_articulo, fecha_orig desc);
create index if not exists idx_retorno_folio on ventas(folio_unico);
create index if not exists idx_retorno_ref on ventas(factura_ref_serie, factura_ref_nro);
