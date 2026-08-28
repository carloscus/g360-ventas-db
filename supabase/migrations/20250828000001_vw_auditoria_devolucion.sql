-- Auditoria: precio atendido vs neto tras NC descuento
-- Para devolucion posterior usar precio_neto (atendido - descuento prorrateado por cantidad_fae)
create or replace view public.vw_auditoria_devolucion as
select
  f.folio_unico               as factura,
  f.fecha_orig                as fecha_factura,
  f.cantidad                  as cant_facturada,
  f.precio_unitario           as precio_atendido,
  f.soles                     as total_factura,
  n.folio_unico               as nc_descuento_ref,
  n.cantidad_fae              as base_descuento_u,
  n.soles                     as monto_descuento,
  round(n.soles/nullif(n.cantidad_fae,0),4) as descuento_unit,
  round(f.precio_unitario - coalesce(n.soles/nullif(n.cantidad_fae,0),0),4) as precio_neto,
  round(f.precio_unitario - coalesce(n.soles/nullif(n.cantidad_fae,0),0),4) as simulacion_1u_neto,
  f.mes_ref
from ventas f
left join ventas n on n.factura_ref_serie=f.serie_doc
  and n.factura_ref_nro=f.nro_doc and n.tipo_operacion='ajuste_valor'
where f.tpo_doc like 'F01%';
comment on view public.vw_auditoria_devolucion is 'Auditoria: precio atendido vs neto tras NC descuento. Para devolucion posterior usar precio_neto.';
