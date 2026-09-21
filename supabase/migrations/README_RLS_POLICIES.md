# RLS Policies - Vistas Internas

## Archivos

| Archivo | Propósito |
|---|---|
| `supabase/migrations/20260915000000_vw_anon_policies.sql` | Migration que crea policies RLS para las 4 vistas |
| `scripts/verify_rls_policies.sql` | Script de verificación post-migration |

## Vistas afectadas

- `public.vw_dim_cliente`
- `public.vw_nc_totales`
- `public.vw_nc_parciales`
- `public.vw_facturas_disponibles`

## Configuración aplicada

```sql
-- Políticas RLS
FOR SELECT TO anon USING (true);

-- Security invoker
ALTER VIEW ... SET (security_invoker = true);
```

## Pasos para aplicar

### 1. Desde Supabase Dashboard
- Ir a **SQL Editor**
- Ejecutar el contenido de `supabase/migrations/20260915000000_vw_anon_policies.sql`

### 2. O usando CLI
```bash
supabase db push
```

### 3. Verificar
```bash
# Ejecutar el script de verificación
supabase db execute scripts/verify_rls_policies.sql
```

## Comportamiento esperado

| Rol | Acceso vw_dim_cliente |
|---|---|
| `anon` | ✅ SELECT permitido |
| `authenticated` | ✅ SELECT permitido |
| Sin autenticar (sin supabase key) | ❌ denied |

## Nota de seguridad

Este proyecto es **entorno interno** con:
- Una sola app consumiendo datos
- Datos no sensibles expuestos (RSU, aggregates)
- URL de proyecto conocida solo internamente

Por eso se permite acceso `anon`. Si en el futuro se requiere mayor seguridad, cambiar a `authenticated` y agregar condiciones `USING`.
