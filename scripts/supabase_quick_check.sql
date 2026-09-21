-- Quick Reference: Supabase Setup for Dual Apps
-- Run this to verify current state after migrations

-- 1. Check table RLS policies
SELECT 
    tablename,
    policyname,
    roles::text,
    cmd
FROM pg_policies
WHERE tablename = 'ventas'
ORDER BY cmd, policyname;

-- 2. Check view RLS policies
SELECT 
    tablename as viewname,
    policyname,
    roles::text,
    cmd
FROM pg_policies
WHERE tablename LIKE 'vw_%'
ORDER BY tablename, policyname;

-- 3. Check security_invoker on views
SELECT 
    viewname,
    security_invoker
FROM pg_views
WHERE schemaname = 'public' 
  AND viewname LIKE 'vw_%'
ORDER BY viewname;

-- 4. Test anon access to views (should work)
SET role TO anon;
SELECT 'vw_dim_cliente' as vista, COUNT(*) FROM public.vw_dim_cliente
UNION ALL
SELECT 'vw_nc_totales', COUNT(*) FROM public.vw_nc_totales
UNION ALL
SELECT 'vw_nc_parciales', COUNT(*) FROM public.vw_nc_parciales
UNION ALL
SELECT 'vw_facturas_disponibles', COUNT(*) FROM public.vw_facturas_disponibles;

-- 5. Test authenticated access to table (should work for Tauri)
SET role TO authenticated;
SELECT COUNT(*) as ventas_count FROM public.ventas;

-- 6. Verify grants
SELECT 
    grantee,
    table_name,
    privilege_type
FROM information_schema.table_privileges
WHERE table_schema = 'public'
  AND grantee = 'anon'
ORDER BY table_name;
