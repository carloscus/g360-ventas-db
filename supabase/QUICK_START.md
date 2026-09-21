# Quick Start - Configurar Supabase para Dual App

## 1️⃣ Ejecutar Migrations en Supabase Dashboard

Ir a **SQL Editor** y ejecutar en orden:

```bash
# Opción A: Copiar y pegar cada archivo
supabase/migrations/20260915000002_rls_consolidated.sql

# Opción B: Usar CLI (si tienes supabase instalado)
supabase db push
```

## 2️⃣ Verificar Estado

```bash
# Ejecutar query de verificación
supabase db remote exec scripts/supabase_quick_check.sql
```

O copiar el contenido de `scripts/supabase_quick_check.sql` al SQL Editor.

## 3️⃣ Obtener Keys

Dashboard → Settings → API

| Key | Valor | Uso |
|---|---|---|
| `anon` | `eyJ...` | PWA Svelte (lectura) |
| `service_role` | `eyJ...` | Tauri Desktop (escritura) |

## 4️⃣ Configurar Apps

### Tauri Desktop
```json
// config.json
{
  "supabase": {
    "url": "https://TU_PROJECT.supabase.co",
    "key": "TU_ANON_KEY",
    "service_role_key": "TU_SERVICE_ROLE_KEY"
  }
}
```

### PWA Svelte
```typescript
// src/lib/supabase.ts
const SUPABASE_URL = 'https://TU_PROJECT.supabase.co'
const SUPABASE_ANON = 'TU_ANON_KEY'

export const supabase = createClient(SUPABASE_URL, SUPABASE_ANON)
```

## 5️⃣ Validar

- [ ] PWA puede leer vistas: `SELECT * FROM vw_dim_cliente`
- [ ] Tauri puede escribir tabla: `INSERT INTO ventas (...)`
- [ ] Anon NO puede escribir: `INSERT INTO ventas` → denied

---

## Vistas Disponibles para PWA

| Vista | Uso |
|---|---|
| `vw_dim_cliente` | Lista de clientes con RUC, nombre, ubicación |
| `vw_nc_totales` | Notas de crédito totales por factura |
| `vw_nc_parciales` | Notas de crédito parciales |
| `vw_facturas_disponibles` | Facturas por cobrar |
| `vw_historial_venta_cliente` | Historial de ventas por cliente |
| `vw_radar_recompra` | Materialized view para recompra |
