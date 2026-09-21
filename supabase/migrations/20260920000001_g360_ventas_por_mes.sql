-- Conteo de filas por mes en Supabase (para sembrar el estado de reconciliacion
-- cuando la BD local se recrea y se perdio sync_months).
-- Consumida via PostgREST: POST /rest/v1/rpc/g360_ventas_por_mes  (body {})
create or replace function public.g360_ventas_por_mes()
returns table(mes_ref text, filas bigint)
language sql
stable
as $$
  select mes_ref, count(*)::bigint as filas
  from public.ventas
  group by mes_ref;
$$;

grant execute on function public.g360_ventas_por_mes() to anon, authenticated, service_role;
