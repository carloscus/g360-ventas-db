-- Verify RLS policies are correctly configured
-- Run this after applying the migration

-- Check views exist and their policies
SELECT 
    p.schemaname,
    p.tablename as viewname,
    p.policyname,
    p.roles,
    p.cmd,
    p.qual as using_clause
FROM pg_policies p
WHERE p.tablename IN ('vw_dim_cliente', 'vw_nc_totales', 'vw_nc_parciales', 'vw_facturas_disponibles')
ORDER BY p.tablename, p.policyname;

-- Test anon access (should work now)
SET role TO anon;
SELECT 'vw_dim_cliente' as vista, COUNT(*) FROM public.vw_dim_cliente
UNION ALL
SELECT 'vw_nc_totales', COUNT(*) FROM public.vw_nc_totales
UNION ALL
SELECT 'vw_nc_parciales', COUNT(*) FROM public.vw_nc_parciales
UNION ALL
SELECT 'vw_facturas_disponibles', COUNT(*) FROM public.vw_facturas_disponibles;
