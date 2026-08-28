-- Seguridad mejorada: key anon = solo lectura, service role = escritura
-- Ejecutar desde Supabase Dashboard → SQL Editor
--
-- Cambio clave:
--   - anon: SELECT únicamente
--   - authenticated: INSERT, UPDATE, DELETE (lo usa el uploader de Tauri
--     cuando se configura el service role key)

alter table public.ventas enable row level security;

-- Borrar politicas existentes que permiten writes a anon
drop policy if exists "ventas_select_anon" on public.ventas;
drop policy if exists "ventas_insert_anon" on public.ventas;
drop policy if exists "ventas_update_anon" on public.ventas;
drop policy if exists "ventas_delete_auth" on public.ventas;

-- SELECT: permite a anon y authenticated leer (frontend, apps downstream)
create policy "ventas_select_readonly"
  on public.ventas for select
  to anon, authenticated
  using (true);

-- INSERT: solo authenticated (Tauri backend con service role key)
create policy "ventas_insert_writer"
  on public.ventas for insert
  to authenticated
  with check (true);

-- UPDATE: solo authenticated
create policy "ventas_update_writer"
  on public.ventas for update
  to authenticated
  using (true) with check (true);

-- DELETE: solo authenticated
create policy "ventas_delete_writer"
  on public.ventas for delete
  to authenticated
  using (true);
