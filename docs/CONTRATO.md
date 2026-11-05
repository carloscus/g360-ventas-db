# CONTRATO — g360-ventas-db

> Contrato de datos para apps consumidoras (cartucho offline) y Supabase (API online).
> **Última versión: `CONTRACTO_VERSION = 4`** (2026-11).

Este documento define **qué** consume una app externa y **cómo** verifica que los
datos son válidos. No describe el motor interno: eso vive en `docs/LINEAMIENTOS.md`.

---

## 1. Dos canales de consumo

| Canal | Formato | Uso | Conexión |
|-------|---------|-----|----------|
| **Cartucho** | `.db` (SQLite) + `.manifest.json` | Apps offline, clientes sin red | Copia de archivo |
| **Supabase** | PostgREST (tablas + vistas) | Apps online, CRM, dashboards | HTTPS + API key |

Ambos canales comparten el **mismo esquema de la tabla `ventas`** y las mismas
reglas de normalización. La única diferencia es que el cartucho es un espejo
completo (raw-first, todas las líneas) y Supabase aplica filtro de líneas +
retención.

---

## 2. Versionado del contrato

Tres constantes controlan compatibilidad. Vive en `src/db/schema.rs`:

| Constante | Valor | Qué controla | ¿Bloquea consumidor? |
|-----------|-------|--------------|----------------------|
| `CONTRACTO_VERSION` | `4` | Formato de tabla `ventas` + tablas auditables | **Sí** — si difiere, rechazar |
| `DIM_SCHEMA_VERSION` | `3` | Semántica de `dim_*` | No — solo reconstruir dims |
| `NORMALIZACION_VERSION` | `1` | Reglas de saneamiento | No — informativo |

### Dónde se lee

```
PRAGMA user_version  == DIM_SCHEMA_VERSION      (3)
g360_meta.contracto_version == CONTRACTO_VERSION (4)
g360_meta.dim_schema_version == DIM_SCHEMA_VERSION (3)
g360_meta.normalizacion_version == NORMALIZACION_VERSION (1)
```

### Regla para consumidores

```
SI  manifest.contracto_version != 4  →  RECHAZAR cartucho (incompatible)
SI  PRAGMA user_version != 3        →  RECONSTRUIR dim_* (no fatal)
SI  normalizacion_version != 1      →  OK (informativo)
```

---

## 3. Cartucho offline (`.db` + `.manifest.json`)

### 3.1 Archivos

| Archivo | Descripción |
|---------|-------------|
| `cartucho_*.db` | SQLite, tabla `ventas` completa (espejo raw-first) |
| `cartucho_*.manifest.json` | Metadatos, checksums, SHA256 del `.db` |

### 3.2 Manifest — campos

```jsonc
{
  "version": 4,                       // CONTRACTO_VERSION (entero)
  "contracto_version": "4",           // string, comparar con consumidor
  "dim_schema_version": "3",
  "normalizacion_version": "1",
  "format": "g360-ventas-cartucho-v2",// identificador de formato
  "tipo": "semilla" | "ejemplo",
  "generado_en": "2026-11-05T02:21:16Z",
  "generado_por": "g360-db-ventas/0.1.0",
  "rango": { "desde": "2010-01-01", "hasta": "2026-11-05" },
  "allowed_lines": ["01","02", ... ], // allowlist usada al generar
  "retention_days": 1277,             // retención Supabase al generar
  "sha256": "hex...",                 // SHA256 del .db
  "db_size_bytes": 2460000000,
  "quick_check": { "ok": true, "filas": 2828634, "soles": 0.0, "meses": 203 },
  "data_quality": { "status": "ok", ... },
  "attributes_applied": ["..."],      // reglas aplicadas
  "mes_checksums": [ { "mes_ref": "2010-01", "checksum": "...", "filas": N, "soles": X } ],
  "day_checksums": [ { "2026-11-01": { "filas": N, "soles": X, "checksum": "..." } } ],
  "espejo_completo": true
}
```

### 3.3 Verificación mínima (recomendada)

```python
import json, hashlib, sqlite3

man = json.load(open("cartucho.manifest.json"))

# 1) Versión de contrato
assert man["contracto_version"] == "4", "cartucho incompatible"

# 2) SHA256 del .db
sha = hashlib.sha256(open("cartucho.db", "rb").read()).hexdigest()
assert sha == man["sha256"], "cartucho corrupto"

# 3) Conteo rápido
conn = sqlite3.connect("cartucho.db")
n = conn.execute("SELECT COUNT(*) FROM ventas").fetchone()[0]
assert n == man["quick_check"]["filas"], "conteo no coincide"

# 4) Meses con checksum
meses = conn.execute("SELECT COUNT(DISTINCT mes_ref) FROM ventas").fetchone()[0]
print(f"OK: {n} filas, {meses} meses, contrato v{man['contracto_version']}")
```

### 3.4 Uso vía CLI

```bash
# Verificar un cartucho contra su manifest
compare_checksums --db cartucho.db --output check.json
```

---

## 4. Tabla `ventas` — esquema (contrato v4)

53 columnas. Las **esenciales para Supabase** están marcadas ★.

| # | Columna | Tipo | ★ | Notas |
|---|---------|------|---|-------|
| 1 | `id` | INTEGER PK | | Autonumérico |
| 2 | `id_articulo` | TEXT | ★ | SKU canónico |
| 3 | `original_sku` | TEXT | | SKU original ERP |
| 4 | `nom_articulo` | TEXT | ★ | |
| 5 | `id_linea` | TEXT | ★ | Normalizado: quita prefijo `01` |
| 6 | `nom_linea` | TEXT | ★ | |
| 7-12 | `id_grupo`,`nom_grupo`,`id_tipo`,`nom_tipo`,`id_familia`,`nom_familia` | TEXT | | |
| 13 | `id_cliente` | TEXT | ★ | Normalizado: `zfill(8)` |
| 14 | `doc_cliente` | TEXT | ★ | RUC(11) / DNI(8) |
| 15 | `nom_cliente` | TEXT | | |
| 16 | `tpo_doc` | TEXT | ★ | F01, NCR, NDB, BDI, FDI, FEX |
| 17 | `serie_doc` | TEXT | ★ | |
| 18 | `nro_doc` | TEXT | ★ | |
| 19 | `referencia` | TEXT | | |
| 20 | `moneda` | TEXT | | default `'Soles'` |
| 21 | `cantidad` | REAL | ★ | Negativa en NCR |
| 22 | `soles` | REAL | ★ | |
| 23 | `dolares` | REAL | | |
| 24 | `precio_unitario` | REAL | | Calculable: `soles/cantidad` |
| 25 | `cantidad_fae` | REAL | | >0 en NDB (descuento en valor) |
| 26 | `anho` | INTEGER | | 2000-2100 |
| 27 | `mes` | INTEGER | | 1-12 |
| 28 | `fecha_orig` | DATE | ★ | ISO `yyyy-mm-dd` |
| 29-30 | `fecha_ref`,`fecha_venc` | TEXT | | |
| 31-35 | `cod_sucursal`,`nom_sucursal`,`departamento`,`provincia`,`distrito` | TEXT | | |
| 36 | `id_vendedor` | TEXT | ★ | Normalizado: quita prefijo `01` |
| 37 | `nom_vendedor` | TEXT | | |
| 38 | `id_pedido` | TEXT | | Espacios → `_` |
| 39 | `ord_compra` | TEXT | | |
| 40 | `file_source` | TEXT | | |
| 41 | `mes_ref` | TEXT | ★ | `yyyy-mm`, derivado de `fecha_orig` |
| 42 | `capturado_en` | TEXT | | |
| 43 | `tipo_operacion` | TEXT | ★ | `venta`/`devolucion`/`ajuste_valor` |
| 44 | `factura_ref_serie` | TEXT | ★ | Serie factura referenciada (NC/ND) |
| 45 | `factura_ref_nro` | TEXT | ★ | Nro factura referenciada |
| 46 | `folio_unico` | TEXT | ★ | `tpo_doc/serie-nro` (clave dedup) |
| 47-53 | `id_ubigeo`,`canal_distribucion`,`nom_condicion_pago`,`estado_linea`,`division`,`fec_cargo`,`id_guia` | TEXT | | Solo local (raw-first) |

---

## 5. Reglas de normalización (NORMALIZACION_VERSION = 1)

| Campo | Regla | Ejemplo entrada → salida |
|-------|-------|--------------------------|
| `id_cliente` | `zfill(8)` | `56101` → `00056101` |
| `id_linea` | quita prefijo `01` si len>2 | `0178` → `78` |
| `id_vendedor` | quita prefijo `01` si len>3 | `01M05` → `M05` |
| `id_pedido` | espacios → `_` | `SADIE 25` → `SADIE_25` |
| `fecha_orig` | a ISO `yyyy-mm-dd` | `01/11/2026` → `2026-11-01` |
| `mes_ref` | derivado de `fecha_orig` | `2026-11-01` → `2026-11` |

### Para búsquedas desde la app

Un consumidor que recibe `id_cliente = "10214"` del usuario debe normalizarlo
antes del `WHERE`:

```
WHERE id_cliente = zfill(8, '10214')   →  '00010214'
WHERE id_linea = lstrip('0178', '01')  →  '78'
```

---

## 6. Clasificación de `tipo_operacion`

Derivada de `tpo_doc` + signo de `cantidad` (`derivar_campos` en parser):

| `tpo_doc` | Condición | `tipo_operacion` | Significado |
|-----------|-----------|------------------|-------------|
| `NCR` | `cantidad < 0` | `devolucion` | Nota crédito — devolución física |
| `NCR` | `cantidad >= 0` | `ajuste_valor` | Nota crédito — ajuste |
| `NDB` | (siempre) | `ajuste_valor` | Nota débito — descuento en valor |
| `F01` | (siempre) | `venta` | Factura (RUC, 11 dígitos) |
| `BDI` | (siempre) | `venta` | Boleta (DNI, 8 dígitos) |
| `FDI`/`FEX` | (siempre) | `venta` | Otros comprobantes |

**Derivados adicionales:**
- `factura_ref_serie/nro`: extraídos de `referencia` (`SERIE-NRO`)
- `folio_unico`: `"{tpo_doc}/{serie_doc}/{nro_doc}"` — clave única de dedup

---

## 7. Supabase (API online)

### 7.1 Conexión

```
URL:          https://<project>.supabase.co
anon key:     solo lectura (apps cliente)
service_role: escritura (solo backend g360)
```

### 7.2 Tablas y vistas

| Recurso | Tipo | Acceso | Uso |
|---------|------|--------|-----|
| `ventas` | tabla | anon=SELECT, service=ALL | Datos crudos (filtrados) |
| `vw_dim_cliente` | vista | anon SELECT | Dimensión cliente (nombre más reciente) |
| `vw_dim_articulo` | vista | anon SELECT | Dimensión artículo |
| `vw_nc_totales` | vista | anon SELECT | Notas de crédito totales |
| `vw_nc_parciales` | vista | anon SELECT | NC parciales |
| `vw_facturas_disponibles` | vista | anon SELECT | Facturas para cruce NC |
| `vw_comparativo_ventas` | vista | anon SELECT | Comparativo YoY/MoM por SKU |
| `vw_comparativo_linea` | vista | anon SELECT | Comparativo por línea |
| `vw_historial_articulo` | vista | anon SELECT | Evolución mensual por SKU |
| `vw_radar_recompra` | MV | anon SELECT | Radar de recompra (refrescar con pg_cron) |

### 7.3 Endpoints (PostgREST)

```
# Ventas filtradas
GET /rest/v1/ventas?mes_ref=eq.2026-11&select=id_articulo,soles

# Vista comparativa
GET /rest/v1/vw_comparativo_ventas?id_linea=eq.01&order=mes_ref

# Vista clientes
GET /rest/v1/vw_dim_cliente?id_cliente=eq.00056101
```

### 7.4 Filtro de líneas (allowlist)

Supabase **no** recibe todas las líneas. El uploader aplica:

```sql
WHERE id_linea IN (allowlist)
   OR (tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%')
      AND factura_referenciada_tiene_linea_permitida
```

**Cambio de allowlist:** re-subir (no re-capturar). Ver `config.allowed_lines`.

### 7.5 Retención

Supabase conserva solo los últimos `retention_days` (default 1277 = 3.5 años).
Monitorizar:

```sql
SELECT * FROM ventas_storage_stats;  -- pct_of_500mb
```

Plan free: 500 MB. Si `pct > 80`, reducir retención o archivar meses.

---

## 8. Checklist de integración para app nueva

```
[ ] 1. Recibir cartucho (.db + .manifest.json) o credenciales Supabase
[ ] 2. Verificar contracto_version == 4  → si no, rechazar
[ ] 3. Verificar SHA256 del .db contra manifest
[ ] 4. Verificar COUNT(*) == manifest.quick_check.filas
[ ] 5. Al buscar: normalizar id_cliente (zfill 8), id_linea/vendedor (quitar 01)
[ ] 6. Para NC/ND: usar tipo_operacion + factura_ref_serie/nro para el cruce
[ ] 7. Clasificar: cantidad<0 → devolución; NDB → ajuste_valor
[ ] 8. Precios: calcular soles/cantidad si precio_unitario es 0
```

---

## 9. Historial de versiones

| Versión | Fecha | Cambio |
|---------|-------|--------|
| **4** | 2026-11 | +`g360_meta`, `day_checksums`, `etl_state`. NDB→`ajuste_valor`. Normalización zfill/lstrip. |
| 3 | 2026-09 | +vistas comparativas, RLS consolidado, MV radar |
| 2 | 2025-09 | +vistas retorno/devolución, fix folio_unico |
| 1 | 2025-08 | Creación tabla `ventas` + dims |
