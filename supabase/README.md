# Supabase — Ventas DB

Capa derivada aislada para Consumo Masivo. Ver `migrations/20250825000000_create_ventas.sql`.

## Aplicar migración

### Opción A — Dashboard (sin CLI)

1. Supabase Dashboard → SQL Editor → New query
2. Pegar contenido de cada migration en orden:
   - `20250825000000_create_ventas.sql` (tabla + indices)
   - `20250828000001_vw_auditoria_devolucion.sql` (vista auditoría)
   - `20250828000003_vw_retornos.sql` (vistas retorno + indices)
   - `20250828000004_rls_readonly_anon.sql` (**seguridad**: anon solo lectura)
3. Verificar: `select * from ventas_storage_stats;`

### Opción B — CLI

```bash
npx supabase link --project-ref TU_REF
npx supabase db push
```

## Modelo de seguridad (RLS)

| Rol | Permisos | Uso |
|-----|----------|-----|
| `anon` | **SELECT only** | Frontend Tauri, apps downstream (nc-sustentor, PWA, etc.) |
| `authenticated` (service role key) | INSERT / UPDATE / DELETE | Backend Rust (uploader) |

> La key service role bypassa RLS por diseño. La anon key respeta todas las políticas.
> Columnas sensibles: `doc_cliente`, `nom_cliente`.

**Migración crítica**: ejecutar `20250828000004_rls_readonly_anon.sql` para restringir writes al service role.

## Variables

En la app Tauri (`%APPDATA%\g360-db-ventas\data\config.json` o `.env`):

```
SUPABASE_URL=https://xxx.supabase.co
SUPABASE_ANON_KEY=eyJ...         # solo lectura
SUPABASE_SERVICE_ROLE_KEY=eyJ... # solo backend Rust (bypass RLS)
```

Tabla: `ventas` (42 cols + id). **Sin constraint única** — la deduplicación se maneja client-side por `(folio_unico, id_articulo)` en cada batch (permite facturas multi-línea). El uploader aplica retención filtrando `WHERE mes_ref >= cutoff` en el SELECT (4 años por defecto).

## Monitoreo capacidad (500 MB plan)

```sql
select * from ventas_storage_stats;
-- total_rows | total_size | table_size | indexes_size | pct_of_500mb
```

Si `pct_of_500mb > 80`, ajustar retención (`supabase_retention_years` en config, default 4) o archivar meses vía UI 📅 meses.

## Vistas disponibles

| Vista | Endpoint PostgREST |
|-------|-------------------|
| `vw_dim_articulo` | `/rest/v1/vw_dim_articulo` |
| `vw_dim_cliente` | `/rest/v1/vw_dim_cliente` |
| `vw_nc_totales` | `/rest/v1/vw_nc_totales` |
| `vw_nc_parciales` | `/rest/v1/vw_nc_parciales` |
| `vw_facturas_disponibles` | `/rest/v1/vw_facturas_disponibles` |
| `vw_auditoria_devolucion` | `/rest/v1/vw_auditoria_devolucion` |

Todas son SELECT-only accesibles desde la anon key.

## Verificación upload

Desde la app: botón **☁ Subir a Supabase** (incremental) o **🔄 Forzar Full Sync** (resetea marcador + re-sube ventana completa). Ambos suben en batches de 500 con service role key y deduplicación por `(folio_unico, id_articulo)`.

Desde CLI:

```bash
cargo run --bin upload
```
