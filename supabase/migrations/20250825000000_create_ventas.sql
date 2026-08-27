-- Ventas DB — tabla derivada aislada (Consumo Masivo)
-- Fuente: SQLite local historial.db (42 cols + id/capturado_en)
-- Destino: Supabase Postgres — capa para CRM, sin modificar origen corporativo

create table if not exists public.ventas (
  id bigserial primary key,

  -- Producto
  id_articulo text not null,
  original_sku text,
  nom_articulo text,
  id_linea text not null,
  nom_linea text,
  id_grupo text,
  nom_grupo text,
  id_tipo text,
  nom_tipo text,
  id_familia text,
  nom_familia text,

  -- Cliente (RUC en doc_cliente, potencialmente sensible)
  id_cliente text not null,
  doc_cliente text,
  nom_cliente text,

  -- Documento
  tpo_doc text not null,
  serie_doc text,
  nro_doc text,
  referencia text,
  moneda text default 'Soles' check (moneda in ('Soles','Dolares')),

  -- Montos
  cantidad double precision not null check (cantidad >= 0),
  soles double precision not null,
  dolares double precision,
  precio_unitario double precision,

  -- Fechas
  anho int not null check (anho between 2000 and 2100),
  mes int not null check (mes between 1 and 12),
  fecha_orig date not null,
  fecha_ref text,
  fecha_venc text,

  -- Ubicacion
  cod_sucursal text,
  nom_sucursal text,
  departamento text,
  provincia text,
  distrito text,

  -- Vendedor / Pedido
  id_vendedor text,
  nom_vendedor text,
  id_pedido text,

  -- Metadata
  file_source text,
  mes_ref text not null,
  capturado_en timestamptz default now(),

  -- Campos derivados para cruce CRM
  tipo_operacion text default 'venta' check (tipo_operacion in ('venta','devolucion','ajuste_valor','nota_debito')),
  factura_ref_serie text,
  factura_ref_nro text,
  folio_unico text not null,

  constraint uq_ventas_folio_unico unique (folio_unico)
);

-- Indices para consultas CRM (igual que SQLite)
create index if not exists idx_venta_mes on public.ventas (mes_ref);
create index if not exists idx_venta_cliente on public.ventas (id_cliente);
create index if not exists idx_venta_doc_cliente on public.ventas (doc_cliente);
create index if not exists idx_venta_sku on public.ventas (id_articulo);
create index if not exists idx_venta_orig_sku on public.ventas (original_sku);
create index if not exists idx_venta_linea on public.ventas (id_linea);
create index if not exists idx_venta_doc on public.ventas (tpo_doc, serie_doc, nro_doc);
create index if not exists idx_venta_ref on public.ventas (referencia);
create index if not exists idx_venta_tipo_op on public.ventas (tipo_operacion);
create index if not exists idx_venta_fact_ref on public.ventas (factura_ref_serie, factura_ref_nro);
create index if not exists idx_venta_fecha on public.ventas (fecha_orig);
create index if not exists idx_venta_anho_mes on public.ventas (anho, mes);

-- Comentarios de columnas sensibles
comment on column public.ventas.doc_cliente is 'RUC/DNI cliente — dato sensible, acceso restringido por RLS';
comment on column public.ventas.nom_cliente is 'Nombre cliente — dato sensible';
comment on column public.ventas.folio_unico is 'Clave natural para upsert onConflict';

-- Habilitar RLS — minimo privilegio
alter table public.ventas enable row level security;

-- Politica: anon con key puede leer (para app Tauri y CRM)
-- Ajustar segun tu modelo: si usas auth.users, cambia a authenticated
drop policy if exists "ventas_select_anon" on public.ventas;
create policy "ventas_select_anon"
  on public.ventas for select
  to anon, authenticated
  using (true);

drop policy if exists "ventas_insert_anon" on public.ventas;
create policy "ventas_insert_anon"
  on public.ventas for insert
  to anon, authenticated
  with check (true);

drop policy if exists "ventas_update_anon" on public.ventas;
create policy "ventas_update_anon"
  on public.ventas for update
  to anon, authenticated
  using (true) with check (true);

-- Para delete restringido, solo authenticated (no anon)
drop policy if exists "ventas_delete_auth" on public.ventas;
create policy "ventas_delete_auth"
  on public.ventas for delete
  to authenticated
  using (true);

-- Vista para monitoreo de capacidad (500 MB plan)
-- Uso: select * from ventas_storage_stats;
create or replace view public.ventas_storage_stats as
select
  (select count(*) from public.ventas) as total_rows,
  pg_size_pretty(pg_total_relation_size('public.ventas')) as total_size,
  pg_size_pretty(pg_relation_size('public.ventas')) as table_size,
  pg_size_pretty(pg_indexes_size('public.ventas')) as indexes_size,
  -- Estimacion vs limite 500 MB
  round(100.0 * pg_total_relation_size('public.ventas') / (500*1024*1024), 1) as pct_of_500mb;
