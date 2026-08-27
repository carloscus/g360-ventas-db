# Supabase — Ventas DB

Capa derivada aislada para Consumo Masivo. Ver `migrations/20250825000000_create_ventas.sql`.

## Aplicar migración

### Opción A — Dashboard (sin CLI)

1. Supabase Dashboard → SQL Editor → New query
2. Pegar contenido de `migrations/20250825000000_create_ventas.sql` → Run
3. Verificar: `select * from ventas_storage_stats;`

### Opción B — CLI

```bash
npx supabase link --project-ref TU_REF
npx supabase db push
```

## Variables

En la app Tauri (`%APPDATA%\g360-db-ventas\data\config.json` o `.env`):

```
SUPABASE_URL=https://xxx.supabase.co
SUPABASE_ANON_KEY=eyJ...
```

Tabla: `ventas` (42 cols + id). Upsert por `folio_unico` (`onConflict=folio_unico`).

## RLS

- `select/insert/update`: `anon, authenticated` (app Tauri usa anon key)
- `delete`: solo `authenticated`
- Columnas sensibles: `doc_cliente`, `nom_cliente`

Ajustar políticas si usas `auth.users` por área.

## Monitoreo capacidad (500 MB plan)

```sql
select * from ventas_storage_stats;
-- total_rows | total_size | table_size | indexes_size | pct_of_500mb
```

Si `pct_of_500mb > 80`, considerar retención (`data_retention_years` en config) o archivar meses vía UI 📅 meses.

## Verificación upload

Desde la app: botón **sinc con supabase** → sube en batches de 500.

Desde CLI:

```bash
cargo run --bin upload
```
