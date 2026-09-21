-- Migration: RLS Policies for internal app views (anon access)
-- Views: vw_dim_cliente, vw_nc_totales, vw_nc_parciales, vw_facturas_disponibles
-- Note: RLS policies work on views without explicit ENABLE ROW LEVEL SECURITY

-- Drop existing policies if any
DROP POLICY IF EXISTS "anon_select" ON public.vw_dim_cliente;
DROP POLICY IF EXISTS "anon_select" ON public.vw_nc_totales;
DROP POLICY IF EXISTS "anon_select" ON public.vw_nc_parciales;
DROP POLICY IF EXISTS "anon_select" ON public.vw_facturas_disponibles;

-- Create policies for anon access (internal app)
CREATE POLICY "anon_select" ON public.vw_dim_cliente
    FOR SELECT TO anon
    USING (true);

CREATE POLICY "anon_select" ON public.vw_nc_totales
    FOR SELECT TO anon
    USING (true);

CREATE POLICY "anon_select" ON public.vw_nc_parciales
    FOR SELECT TO anon
    USING (true);

CREATE POLICY "anon_select" ON public.vw_facturas_disponibles
    FOR SELECT TO anon
    USING (true);

-- Set security_invoker = true for all views (recommended)
ALTER VIEW public.vw_dim_cliente SET (security_invoker = true);
ALTER VIEW public.vw_nc_totales SET (security_invoker = true);
ALTER VIEW public.vw_nc_parciales SET (security_invoker = true);
ALTER VIEW public.vw_facturas_disponibles SET (security_invoker = true);

-- Verification query
SELECT 
    schemaname, 
    viewname,
    (SELECT COUNT(*) FROM pg_policies WHERE tablename = v.viewname) as policy_count
FROM pg_views v
WHERE schemaname = 'public' 
  AND viewname IN ('vw_dim_cliente', 'vw_nc_totales', 'vw_nc_parciales', 'vw_facturas_disponibles');
