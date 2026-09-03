-- vw_dim_cliente / vw_dim_articulo: el nombre mostrado debe ser el MAS RECIENTE,
-- no el alfabetico. Las definiciones originales usaban MAX(nom_cliente), que al
-- haber un renombre de razon social (ej: "X S.R.L." -> "X S.A.C.") mostraria el
-- nombre viejo si alfabeticamente "ganaba". Con id_cliente/id_articulo como clave
-- estable, el renombre NO corrompe agregaciones: cada fila conserva el nombre
-- vigente al exportarse (historia auditable) y la dimension muestra el actual.
--
-- Contexto 2026-09-02: hoy 0 clientes con 2+ nombres (el ERP estampa el nombre
-- actual del master al exportar; renombres previos a la carga masiva son
-- invisibles). Esta correccion prepara el futuro: cuando un renombre ocurra
-- entre capturas, la dimension mostrara el nombre de la venta mas reciente.

create or replace view public.vw_dim_cliente as
select distinct on (id_cliente)
  id_cliente, doc_cliente, nom_cliente, departamento, provincia, distrito
from public.ventas
where id_cliente != ''
order by id_cliente, fecha_orig desc, id desc;

create or replace view public.vw_dim_articulo as
select distinct on (id_articulo)
  id_articulo, original_sku, nom_articulo, id_linea, nom_linea,
  id_grupo, nom_grupo, id_tipo, nom_tipo, id_familia, nom_familia
from public.ventas
where id_articulo != ''
order by id_articulo, fecha_orig desc, id desc;
