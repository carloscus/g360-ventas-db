-- Fix: security_invoker = true para vistas existentes
-- Solo aplica a vistas que realmente existen

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
            RAISE NOTICE 'Applied to: %', v_viewname;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Skipped %: %', v_viewname, SQLERRM;
        END;
    END LOOP;
END $$;

-- Verification (use pg_class.reloptions)
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
