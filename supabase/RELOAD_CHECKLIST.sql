-- ============================================================================
-- RELOAD CHECKLIST — ejecutar en Supabase Dashboard → SQL Editor
-- Proyecto: cipsa_db_ventas (tqdmoytaucnfrpaklprc)
-- Motivo: la tabla ventas fue recreada manualmente (perdio RLS y parte de los
-- indices) y el churn de full-sync (purga ~600K + re-subida) dejo bloat que
-- provoca statement timeouts (500 / 57014) en operaciones sin filtro.
-- Ejecutar BLOQUE por BLOQUE, en orden. Verificar con el BLOQUE 5.
-- ============================================================================

-- BLOQUE 1 — Seguridad: RLS obligatorio (cierra rls_disabled_in_public)
-- Writes SOLO via service_role (bypassa RLS). anon/authenticated: solo lectura.
alter table public.ventas enable row level security;
alter table public.ventas force row level security;

-- Defensa extra: aunque alguien apague RLS de nuevo, writes siguen cerrados.
revoke insert, update, delete on public.ventas from anon, authenticated;

drop policy if exists "ventas_select_anon" on public.ventas;
drop policy if exists "ventas_select_readonly" on public.ventas;
drop policy if exists "ventas_insert_anon" on public.ventas;
drop policy if exists "ventas_insert_writer" on public.ventas;
drop policy if exists "ventas_update_anon" on public.ventas;
drop policy if exists "ventas_update_writer" on public.ventas;
drop policy if exists "ventas_delete_auth" on public.ventas;
drop policy if exists "ventas_delete_writer" on public.ventas;

create policy "ventas_select_readonly"
  on public.ventas for select
  to anon, authenticated
  using (true);

-- BLOQUE 2 — Indices (si la tabla se recreo a mano, probablemente faltan)
-- Base (igual que migracion 20250825000000):
create index if not exists idx_venta_mes        on public.ventas (mes_ref);
create index if not exists idx_venta_cliente    on public.ventas (id_cliente);
create index if not exists idx_venta_doc_cliente on public.ventas (doc_cliente);
create index if not exists idx_venta_sku        on public.ventas (id_articulo);
create index if not exists idx_venta_orig_sku   on public.ventas (original_sku);
create index if not exists idx_venta_linea      on public.ventas (id_linea);
create index if not exists idx_venta_doc        on public.ventas (tpo_doc, serie_doc, nro_doc);
create index if not exists idx_venta_ref        on public.ventas (referencia);
create index if not exists idx_venta_tipo_op    on public.ventas (tipo_operacion);
create index if not exists idx_venta_fact_ref   on public.ventas (factura_ref_serie, factura_ref_nro);
create index if not exists idx_venta_fecha      on public.ventas (fecha_orig);
create index if not exists idx_venta_anho_mes   on public.ventas (anho, mes);
-- De 20250902000001 (rendimiento cockpit):
create index if not exists idx_venta_vendedor_cliente on public.ventas (id_vendedor, id_cliente);
create index if not exists idx_venta_nro_doc          on public.ventas (nro_doc);
create index if not exists idx_venta_cliente_fecha    on public.ventas (id_cliente, fecha_orig);

-- BLOQUE 3 — Integridad: SIN constraint unica en folio_unico (multi-linea).
-- Si al recrear la tabla volvio uq_ventas_folio_unico, la subida fallaria con
-- 409 en la primera factura multi-linea. Verificar y dropear si existe:
alter table public.ventas drop constraint if exists uq_ventas_folio_unico;
alter table public.ventas drop constraint if exists uq_ventas_folio_sku;

-- BLOQUE 4 — Rendimiento: estadisticas + vacuum del churn del full-sync
-- (purga ~600K + re-subida deja dead tuples; el count sin filtro daba 500/57014)
-- IMPORTANTE: ejecutar cada sentencia en una corrida SEPARADA del SQL Editor.
-- VACUUM no puede correr dentro de una transaccion (error 25001 si vas junto).
analyze public.ventas;
-- ^ Run (corrida 1) — luego, en un query NUEVO, solo:
-- vacuum (analyze) public.ventas;   <- corrida 2, SOLo esa linea

-- BLOQUE 5 — Verificacion
-- 5a. RLS activo + politica de solo lectura (debe listar ventas_select_readonly)
select c.relname, c.relrowsecurity, c.relforcerowsecurity
from pg_class c where c.relname = 'ventas';
select policyname, cmd, roles from pg_policies where tablename = 'ventas';

-- 5b. Sin constraint unica en folio_unico (no debe devolver filas)
select conname from pg_constraint
where conrelid = 'public.ventas'::regclass and contype = 'u';

-- 5c. Indices (deben ser >= 14)
select indexrelname from pg_stat_user_indexes where relname = 'ventas';

-- 5d. Bloat reducido (dead/live bajo tras vacuum; ejecutar de nuevo si > 10%)
select n_live_tup, n_dead_tup,
       round(100.0 * n_dead_tup / greatest(n_live_tup, 1), 1) as pct_dead
from pg_stat_user_tables where relname = 'ventas';

-- 5e. Count sin filtro: antes daba 500 por timeout; debe responder rapido
select count(*) from public.ventas;

-- BLOQUE 6 (opcional) — Monitoreo de capacidad (plan gratuito = 500 MB)
-- Vista instantanea: usa pg_stat (aproximado), NO count(*) (full scan = timeout).
create or replace view public.vw_ventas_storage as
select
  s.n_live_tup                                  as total_rows,
  pg_size_pretty(pg_total_relation_size('public.ventas')) as total_size,
  pg_total_relation_size('public.ventas')       as total_bytes,
  pg_size_pretty(pg_relation_size('public.ventas'))       as table_size,
  pg_size_pretty(pg_indexes_size('public.ventas'))        as indexes_size,
  round(100.0 * pg_total_relation_size('public.ventas') / (500 * 1024 * 1024), 1) as pct_of_500mb
from pg_stat_user_tables s
where s.relname = 'ventas';

-- Consultar despues: select * from vw_ventas_storage;
-- (los numeros son aproximados; ejecutar ANALYZE antes para refrescar)

-- BLOQUE 7 (opcional) — Vacuum semanal automatico via pg_cron
-- La rotacion mensual (un mes entra, un mes sale) genera ~11 MB/mes de dead
-- tuples; autovacuum los reutiliza pero no reduce el archivo. Este cron limpia
-- y refresca estadisticas cada lunes 04:00 UTC (23:00 Peru domingo).
create extension if not exists pg_cron;

select cron.schedule(
  'vacuum-ventas-semanal',
  '0 4 * * 1',
  'VACUUM (ANALYZE) public.ventas'
);

-- Desactivar si alguna vez hace falta:
-- select cron.unschedule('vacuum-ventas-semanal');
