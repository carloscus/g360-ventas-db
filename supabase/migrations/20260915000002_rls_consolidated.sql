-- Migration: Consolidated RLS Security for Dual App Architecture
-- Purpose: Prepare Supabase for TWO apps:
--   1. Tauri Desktop (service_role key) → WRITE to ventas table
--   2. PWA Svelte (anon key) → READ from views only
--
-- NOTE: PostgreSQL does NOT support RLS policies on views.
-- Views inherit security from underlying tables via security_invoker = true.

-- ============================================================================
-- PART 1: BASE TABLE (ventas) - RLS Policies
-- Ensure policies exist for authenticated write + anon read
-- ============================================================================

-- Table must have RLS enabled
ALTER TABLE public.ventas ENABLE ROW LEVEL SECURITY;

-- Drop ALL existing policies on ventas to start fresh
DROP POLICY IF EXISTS "ventas_select_readonly" ON public.ventas;
DROP POLICY IF EXISTS "ventas_insert_writer" ON public.ventas;
DROP POLICY IF EXISTS "ventas_update_writer" ON public.ventas;
DROP POLICY IF EXISTS "ventas_delete_writer" ON public.ventas;
DROP POLICY IF EXISTS "ventas_select_anon" ON public.ventas;
DROP POLICY IF EXISTS "ventas_insert_anon" ON public.ventas;
DROP POLICY IF EXISTS "ventas_update_anon" ON public.ventas;
DROP POLICY IF EXISTS "ventas_delete_auth" ON public.ventas;
DROP POLICY IF EXISTS "enable_read" ON public.ventas;
DROP POLICY IF EXISTS "enable_write" ON public.ventas;
DROP POLICY IF EXISTS "allow_all" ON public.ventas;
DROP POLICY IF EXISTS "Allow all operations" ON public.ventas;
DROP POLICY IF EXISTS "Enable access for all users" ON public.ventas;

-- SELECT: allows both anon and authenticated to read
CREATE POLICY "ventas_select_readonly"
  ON public.ventas
  FOR SELECT
  TO anon, authenticated
  USING (true);

-- INSERT: only authenticated (Tauri backend with service_role)
CREATE POLICY "ventas_insert_writer"
  ON public.ventas
  FOR INSERT
  TO authenticated
  WITH CHECK (true);

-- UPDATE: only authenticated
CREATE POLICY "ventas_update_writer"
  ON public.ventas
  FOR UPDATE
  TO authenticated
  USING (true)
  WITH CHECK (true);

-- DELETE: only authenticated
CREATE POLICY "ventas_delete_writer"
  ON public.ventas
  FOR DELETE
  TO authenticated
  USING (true);

-- ============================================================================
-- PART 2: VIEWS - security_invoker = true
-- This ensures views respect the RLS policies of underlying tables.
-- No CREATE POLICY needed (or allowed) on views.
-- ============================================================================

-- Apply security_invoker = true to ALL views (skip if not exists)
DO $$
DECLARE
    v_viewname text;
BEGIN
    FOR v_viewname IN 
        SELECT viewname FROM pg_views 
        WHERE schemaname = 'public' AND viewname LIKE 'vw_%'
    LOOP
        BEGIN
            EXECUTE format('ALTER VIEW public.%I SET (security_invoker = true)', v_viewname);
            RAISE NOTICE 'Applied security_invoker=true to: %', v_viewname;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Skipped %: %', v_viewname, SQLERRM;
        END;
    END LOOP;
END $$;

-- ============================================================================
-- PART 3: MATERIALIZED VIEWS - Grant SELECT to anon
-- (RLS does not apply to materialized views - use GRANT instead)
-- ============================================================================
GRANT SELECT ON public.vw_radar_recompra TO anon;

-- ============================================================================
-- PART 4: REVOKE direct access to base tables from anon (defense in depth)
-- This prevents bypassing view security by querying tables directly
-- ============================================================================
REVOKE ALL ON public.ventas FROM anon;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM anon;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA public FROM anon;

-- ============================================================================
-- PART 5: VERIFICATION QUERIES
-- Run these to confirm the setup is correct
-- ============================================================================

-- Check table policies
SELECT 
    tablename as object_name,
    policyname,
    roles::text,
    cmd
FROM pg_policies
WHERE tablename = 'ventas'
ORDER BY cmd, policyname;

-- Check view properties (use pg_class.reloptions instead of pg_views.security_invoker)
SELECT 
    v.schemaname,
    v.viewname,
    CASE 
        WHEN c.reloptions IS NULL THEN 'false'
        ELSE ARRAY_TO_STRING(c.reloptions, ',')
    END as options
FROM pg_views v
JOIN pg_class c ON c.relname = v.viewname AND c.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = 'public')
WHERE v.schemaname = 'public' 
  AND v.viewname LIKE 'vw_%'
ORDER BY v.viewname;

-- Check grants on materialized views
SELECT 
    grantee,
    table_name,
    privilege_type
FROM information_schema.table_privileges
WHERE table_schema = 'public'
  AND table_name = 'vw_radar_recompra'
  AND grantee = 'anon';
