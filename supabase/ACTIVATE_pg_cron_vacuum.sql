-- ═══════════════════════════════════════════════════════════════════════════
-- ACTIVAR: VACUUM semanal automático + refresh de materialized views
-- Ejecutar UNA sola vez en: Supabase Dashboard → SQL Editor
-- ═══════════════════════════════════════════════════════════════════════════
--
-- ¿POR QUÉ? La rotación de datos (purga + re-subida en reconcile) genera
-- dead tuples que inflan la tabla (723 MB con 543K filas vivas = bloat).
-- El autovacuum de Postgres reutiliza el espacio PERO NO reduce el archivo,
-- y Supabase cuenta el tamaño total contra el plan free (500 MB).
-- Este cron ejecuta VACUUM (ANALYZE) semanalmente para mantener la cuota.
--
-- PREREQUISITO: habilitar la extensión pg_cron:
--   Dashboard → Database → Extensions → buscar "pg_cron" → Enable
--
-- Verificado hoy (2026-11-05): VACUUM FULL manual llevó 723 MB → 347 MB (69.4%).

-- ── 1) Extensión pg_cron (si no está habilitada, esto fallará con error claro) ──
create extension if not exists pg_cron;

-- ── 2) VACUUM semanal — domingo 22:00 UTC (17:00 Perú, post-sync) ──
-- Nota: no corre dentro de transacción; pg_cron lo ejecuta como sentencia simple.
select cron.schedule(
  'vacuum-ventas-semanal',
  '0 22 * * 0',
  'VACUUM (ANALYZE) public.ventas'
);

-- ── 3) (Opcional) Refresh nocturno de materialized views — 21:00 UTC (16:00 Perú) ──
-- Descomentar si pg_cron está habilitado y quieres frescura diaria de:
--   vw_radar_recompra y mv_venta_linea_sku_mes (base de vw_comparativo_*)
--
-- select cron.schedule(
--   'refresh-mv-ventas',
--   '0 21 * * *',
--   'REFRESH MATERIALIZED VIEW CONCURRENTLY public.vw_radar_recompra; REFRESH MATERIALIZED VIEW CONCURRENTLY public.mv_venta_linea_sku_mes;'
-- );

-- ═══════════════════════════════════════════════════════════════════════════
-- VERIFICACIÓN
-- ═══════════════════════════════════════════════════════════════════════════

-- Ver los jobs programados:
select jobid, schedule, command from cron.job;

-- Ver historial de ejecuciones (tras la primera semana):
select jobid, runid, status, start_time, end_time
from cron.job_run_details
order by start_time desc limit 10;

-- Bloat actual (dead/live ratio) — ideal < 10%:
select
  n_live_tup, n_dead_tup,
  round(100.0 * n_dead_tup / greatest(n_live_tup, 1), 1) as pct_dead
from pg_stat_user_tables
where relname = 'ventas';

-- Tamaño de la tabla:
select * from vw_ventas_storage;

-- ═══════════════════════════════════════════════════════════════════════════
-- DESINSTALAR (si se necesita):
--   select cron.unschedule('vacuum-ventas-semanal');
--   select cron.unschedule('refresh-mv-ventas');
-- ═══════════════════════════════════════════════════════════════════════════
