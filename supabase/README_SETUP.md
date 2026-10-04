# Supabase Setup Guide - Dual App Architecture

## Arquitectura

```
┌─────────────────────┐         ┌─────────────────────┐
│   TAURI APP         │         │   PWA SVELTE        │
│   (Escritorio)      │         │   (Navegador)       │
├─────────────────────┤         ├─────────────────────┤
│ • Captura datos     │         │                     │
│ • SQLite local      │         │  Solo LECTURA       │
│ • Upload a Supabase │         │  desde Views        │
│ • Service role key  │         │  Anon key           │
└──────────┬──────────┘         └──────────┬──────────┘
           │                                │
           │  INSERT/UPDATE/DELETE          │ SELECT
           ▼                                ▼
┌──────────────────────────────────────────────────────┐
│                 SUPABASE                             │
│                                                      │
│  Tabla: ventas (RLS: authenticated write, anon read) │
│                                                      │
│  Vistas (RLS: anon SELECT only):                    │
│    • vw_dim_cliente                                 │
│    • vw_dim_articulo                                │
│    • vw_nc_totales                                  │
│    • vw_nc_parciales                                │
│    • vw_facturas_disponibles                        │
│    • vw_historial_venta_cliente                     │
│    • vw_radar_recompra (materializado)              │
│    • vw_auditoria_devolucion                        │
│    • vw_cockpit_ventas                              │
└──────────────────────────────────────────────────────┘
```

---

## Pasos para Preparar Supabase

### Paso 1: Ejecutar Migrations

Orden cronológico (ejecutar en Supabase SQL Editor):

1. `20250825000000_create_ventas.sql` - Crear tabla ventas
2. `20250828000001_vw_auditoria_devolucion.sql` - Vistas auditoría
3. `20250828000003_vw_retornos.sql` - Vistas NC/retornos
4. `20250828000004_rls_readonly_anon.sql` - RLS tabla ventas
5. `20250901000000_fix_folio_unique.sql` - Fix constraints
6. `20250902000001_query_performance_indexes.sql` - Índices
7. `20250902000002_vw_cockpit_ventas.sql` - Vistas cockpit
8. `20250902000003_mv_radar.sql` - Materialized view
9. `20250902000004_vw_dim_nombre_reciente.sql` - Dim views actualizadas
10. `20260915000002_rls_consolidated.sql` - **RLS consolidado para todas las vistas**

### Paso 2: Verificar Políticas

Ejecutar query de verificación (incluida en migration):

```sql
SELECT 
    tablename as object_name,
    policyname,
    roles::text,
    cmd
FROM pg_policies
WHERE schemaname = 'public'
ORDER BY tablename, policyname;
```

Deberías ver:
- `ventas`: 4 políticas (select, insert, update, delete)
- Cada `vw_*`: 1 política `vw_anon_select` para `anon`

### Paso 3: Obtener Keys

Ir a **Supabase Dashboard → Settings → API**

| Key | Uso | App |
|---|---|---|
| `anon` public key | Solo lectura | PWA Svelte |
| `service_role` private key | Escritura | Tauri Desktop |

⚠️ **IMPORTANTE**: La `service_role` key NO debe exponerse en el frontend.

---

## Configuración de Apps

### Tauri Desktop (Escritorio)

Archivo: `src-tauri/src/main.rs`

```json
// config.json
{
  "supabase": {
    "url": "https://xxx.supabase.co",
    "key": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...", // anon (para test)
    "service_role_key": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..." // service_role (write)
  },
  "intranet": {
    "user": "tu_usuario",
    "pass": "tu_password"
  },
  "allowed_lines": ["01", "02", "09", "11", "14"],
  "supabase_retention_days": 1095,
  "auto_sync": true
}
```

### PWA Svelte (Navegador)

Archivo: `src/lib/supabase.ts` (o config similar)

```typescript
import { createClient } from '@supabase/supabase-js'

const SUPABASE_URL = 'https://xxx.supabase.co'
const SUPABASE_ANON = 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...' // Solo lectura

export const supabase = createClient(SUPABASE_URL, SUPABASE_ANON)

// Ejemplos de uso:

// Get clientes
const { data: clientes } = await supabase
  .from('vw_dim_cliente')
  .select('*')
  .limit(100)

// Get NC totales
const { data: nc } = await supabase
  .from('vw_nc_totales')
  .select('*')

// Get facturas disponibles
const { data: facturas } = await supabase
  .from('vw_facturas_disponibles')
  .select('*')
  .order('fecha_vencimiento', { ascending: true })
  .limit(50)
```

---

## Seguridad

| Role | Acceso tabla `ventas` | Acceso vistas `vw_*` |
|---|---|---|
| `anon` | ❌ SELECT (revocado) | ✅ SELECT |
| `authenticated` | ✅ SELECT/INSERT/UPDATE/DELETE | ✅ SELECT |
| `service_role` | ✅ SELECT/INSERT/UPDATE/DELETE (ignora RLS) | ✅ SELECT |

---

## Troubleshooting

### Error: "permission denied for table vw_xxx"
- Verificar que la política `vw_anon_select` existe
- Ejecutar: `SELECT * FROM pg_policies WHERE tablename = 'vw_xxx'`

### Error: "relation vw_xxx does not exist"
- Verificar que la vista fue creada correctamente
- Ejecutar: `SELECT * FROM pg_views WHERE viewname = 'vw_xxx'`

### Error: "duplicate policy"
- Las políticas ya existen, ejecutar el migration con `DROP POLICY IF EXISTS` primero

---

## Checklist Final

- [ ] Todas las migrations ejecutadas
- [ ] Políticas RLS verificadas en pg_policies
- [ ] Keys de Supabase obtenidas (anon + service_role)
- [ ] Tauri configurado con service_role key
- [ ] PWA configurado con anon key
- [ ] Test de lectura desde PWA funciona
- [ ] Test de escritura desde Tauri funciona
