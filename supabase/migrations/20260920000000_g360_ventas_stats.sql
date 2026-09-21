-- Estadisticas agregadas de la tabla ventas para el dashboard (seccion Supabase).
-- Devuelve: fecha minima/maxima, filas y documentos distintos.
-- Consumida via PostgREST: POST /rest/v1/rpc/g360_ventas_stats  (body {})
create or replace function public.g360_ventas_stats()
returns table(min_fecha date, max_fecha date, filas bigint, documentos bigint)
language sql
stable
as $$
  select
    min(fecha_orig)::date as min_fecha,
    max(fecha_orig)::date as max_fecha,
    count(*)::bigint       as filas,
    count(distinct folio_unico)::bigint as documentos
  from public.ventas;
$$;

grant execute on function public.g360_ventas_stats() to anon, authenticated, service_role;
