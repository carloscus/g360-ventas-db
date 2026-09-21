# Guía de Consumo API — g360-ventas-db

Guía para apps downstream que consumen datos de Supabase vía PostgREST.

## Configuración básica

```python
# Ejemplo Python (requests)
import requests

SUPABASE_URL = "https://tqdmoytaucnfrpaklprc.supabase.co"
ANON_KEY = "eyJ...anon_key"

headers = {
    "apikey": ANON_KEY,
    "Authorization": f"Bearer {ANON_KEY}"
}

def query(view, params=None):
    url = f"{SUPABASE_URL}/rest/v1/{view}"
    resp = requests.get(url, headers=headers, params=params or {})
    resp.raise_for_status()
    return resp.json()
```

```javascript
// Ejemplo JavaScript (fetch)
const SUPABASE_URL = "https://tqdmoytaucnfrpaklprc.supabase.co";
const ANON_KEY = "eyJ...anon_key";

async function query(view, params = {}) {
  const url = new URL(`${SUPABASE_URL}/rest/v1/${view}`);
  Object.entries(params).forEach(([k, v]) => url.searchParams.set(k, v));
  const resp = await fetch(url, {
    headers: { apikey: ANON_KEY, Authorization: `Bearer ${ANON_KEY}` }
  });
  if (!resp.ok) throw new Error(resp.statusText);
  return resp.json();
}
```

## Endpoints disponibles

### Devoluciones

| Endpoint | Descripción | Docs |
|----------|-------------|------|
| `vw_dim_articulo` | Catálogo de SKUs | [api-retornos.md](api-retornos.md) |
| `vw_dim_cliente` | Lookup de clientes | [api-retornos.md](api-retornos.md) |
| `vw_facturas_disponibles` | Facturas con saldo (LIFO) | [api-retornos.md](api-retornos.md) |
| `vw_nc_totales` | NC totales de una factura | [api-retornos.md](api-retornos.md) |
| `vw_nc_parciales` | NC parciales | [api-retornos.md](api-retornos.md) |

### Análisis comparativo

| Endpoint | Descripción | Docs |
|----------|-------------|------|
| `vw_comparativo_ventas` | Variación interanual/mensual por SKU | [api-analisis-comparativos.md](api-analisis-comparativos.md) |
| `vw_comparativo_linea` | Variación interanual/mensual por línea | [api-analisis-comparativos.md](api-analisis-comparativos.md) |
| `vw_historial_venta_cliente` | Histórico de precios por cliente+SKU | [api-analisis-comparativos.md](api-analisis-comparativos.md) |
| `vw_historial_articulo` | Histórico de artículo | [api-analisis-comparativos.md](api-analisis-comparativos.md) |
| `vw_radar_recompra` | Radar de recompra | [api-analisis-comparativos.md](api-analisis-comparativos.md) |
| `mv_venta_linea_sku_mes` | Venta por línea+SKU+mes | [api-analisis-comparativos.md](api-analisis-comparativos.md) |

## Filtros PostgREST

### Operadores comunes

| Operador | Sintaxis | Ejemplo |
|----------|----------|---------|
| Igual | `eq.` | `id_cliente=eq.00068414` |
| Mayor que | `gt.` | `soles=gt.1000` |
| Mayor o igual | `gte.` | `mes_ref=gte.2025-01` |
| Menor que | `lt.` | `soles=lt.100` |
| Menor o igual | `lte.` | `mes_ref=lte.2025-12` |
| No igual | `neq.` | `tipo_operacion=neq.venta` |
| En lista | `in.` | `id_linea=in.(01,02,09)` |
| Contiene | `like.` | `nom_articulo=like.%VINIFAN%` |
| Empieza con | `like.` | `id_articulo=like.01%` |

### Selección de campos

```bash
# Solo campos específicos
?select=id_articulo,nom_articulo,soles

# Contar filas (sin traer datos)
?select=*&limit=0
# Respuesta: header Content-Range: 0-0/541327
```

### Paginación

```bash
# Primeros 100 registros
?select=*&limit=100&offset=0

# Siguientes 100
?select=*&limit=100&offset=100
```

### Ordenamiento

```bash
# Por fecha descendente
?order=fecha_orig.desc

# Por múltiples campos
?order=id_linea.asc,mes_ref.desc
```

## Ejemplos de uso

### 1. Listar SKUs de una línea

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_dim_articulo?select=id_articulo,nom_articulo&id_linea=eq.01&limit=50" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 2. Buscar cliente por RUC

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_dim_cliente?select=*&doc_cliente=eq.20100068414" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 3. Ventas de un cliente en un mes

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/ventas?select=*&id_cliente=eq.00068414&mes_ref=eq.2025-11&tipo_operacion=eq.venta" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 4. Artículos más vendidos por línea

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/mv_venta_linea_sku_mes?select=id_articulo,SUM(cantidad)&id_linea=eq.01&group=id_articulo&order=SUM(cantidad).desc&limit=10" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 5. Devoluciones de un cliente

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/ventas?select=folio_unico,fecha_orig, cantidad,soles&id_cliente=eq.00068414&tipo_operacion=eq.devolucion&order=fecha_orig.desc" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

## Autenticación

Todos los endpoints requieren headers:

```
apikey: {ANON_KEY}
Authorization: Bearer {ANON_KEY}
```

La clave anon está en `config.json` → `supabase.key`.

**Nota:** El rol `anon` solo tiene permisos SELECT (RLS). Las operaciones INSERT/UPDATE/DELETE requieren la service role key (solo para el backend Rust).

## Errores comunes

| Código | Significado | Solución |
|--------|-------------|----------|
| 401 | API key inválida | Verificar `apikey` y `Authorization` |
| 403 | Sin permisos (RLS) | El rol `anon` no tiene acceso a esa tabla |
| 404 | Tabla/vista no existe | Verificar el nombre del endpoint |
| 400 | Query inválida | Revisar sintaxis de filtros PostgREST |

## Rate limits

- Plan gratuito: 500 requests/minuto
- Recomendación: cachear datos estáticos (dims, catálogos)
- Para datos en tiempo real: usar ` Prefer: return=representation` solo cuando sea necesario
