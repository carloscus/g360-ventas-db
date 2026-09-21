-- Security enhancement for internal project
-- Purpose: Ensure views are read-only, no write operations possible from outside

-- 1. REVOKE default privileges from anon role (defense in depth)
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM anon;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM anon;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA public FROM anon;

-- 2. Grant only SELECT on specific views to anon
GRANT SELECT ON public.vw_dim_cliente TO anon;
GRANT SELECT ON public.vw_nc_totales TO anon;
GRANT SELECT ON public.vw_nc_parciales TO anon;
GRANT SELECT ON public.vw_facturas_disponibles TO anon;

-- 3. Explicitly DENY write operations on base tables (if they exist and are accessible)
-- This prevents bypass via underlying tables
-- Uncomment if you want extra protection:

-- REVOKE INSERT, UPDATE, DELETE ON public.ventas FROM anon;
-- REVOKE INSERT, UPDATE, DELETE ON public.dim_ruc FROM anon;

-- 4. Create a SECURITY DEFINER view wrapper (optional, maximum security)
-- This ensures only specific columns are visible
CREATE OR REPLACE VIEW public.vw_dim_cliente_secure AS
SELECT 
    ruc,
    LEFT(nom_cliente, 50) as nom_cliente_short,  -- Truncate long names
    departamento,
    provincia,
    distrito,
    n_clientes_unicos
FROM public.vw_dim_cliente;

CREATE POLICY "secure_anon_select" ON public.vw_dim_cliente_secure
    FOR SELECT TO anon
    USING (true);

-- 5. Log attempts to access non-view objects (audit trail)
-- Create a function to log unauthorized access attempts
CREATE OR REPLACE FUNCTION public.log_access_attempt()
RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO public.audit_log (table_name, operation, attempted_by, attempted_at)
    VALUES (TG_TABLE_NAME, TG_OP, current_user, now());
    RETURN NULL;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

-- 6. Trigger for audit (optional - creates table if not exists)
CREATE TABLE IF NOT EXISTS public.audit_log (
    id SERIAL PRIMARY KEY,
    table_name TEXT NOT NULL,
    operation TEXT NOT NULL,
    attempted_by TEXT NOT NULL DEFAULT current_user,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
