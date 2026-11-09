# Base canónica y consumo por otras apps

`g360-db-ventas` es la **productora canónica** del histórico de ventas (SQLite
`historial.db`). Otras apps **no** deben leer ni escribir el SQLite principal en
caliente: toman una **instantánea** y trabajan sobre su **propia copia**.

## Capas

| Capa | Rol | Quién escribe |
|------|-----|---------------|
| Intranet / ERP | Fuente original | Nadie (externo, no se toca) |
| `historial.db` (SQLite) | Derivado canónico, histórico completo (todas las líneas) | **Solo** `g360-db-ventas` |
| Instantánea (`base_canonica_*.db`) | Copia aislada y portable + manifiesto | Productor (export) |
| App consumidora | Copia propia + tablas/vistas de su dominio | La app consumidora |

## Cómo exportar la instantánea

En la app: módulo **🗄 Cartucho / Backup → 📦 Generar cartucho**. Genera en
`%APPDATA%\g360-db-ventas\data\export\`:

- `base_canonica_<YYYYMMDD_HHMMSS>.db` — archivo autocontenido (`VACUUM INTO`, sin WAL).
- `base_canonica_<YYYYMMDD_HHMMSS>.manifest.json` — metadatos de validación.

> `VACUUM INTO` es la forma correcta de copiar: produce un `.db` consistente aunque
> la productora esté corriendo. **No** copies `historial.db` + `-wal` a mano.

### Desde CLI (USB / carpeta compartida)

`export.exe` genera la instantánea en una ruta — ideal para el flujo manual o automatizado:

```bash
export                        # -> <data>/export/base_canonica_<ts>.db (+ .manifest.json)
export E:\respaldo            # -> E:\respaldo\base_canonica_<ts>.db   (USB)
export "\\servidor\share\g360"# -> carpeta compartida
export D:\snap\corte.db       # -> ruta .db exacta
```

> Es pesado (copia completa vía `VACUUM INTO`, ~1-2 min por GB). Ejecútalo cuando la
> app no esté sincronizando.

## Manifiesto (`*.manifest.json`)

| Campo | Descripción |
|-------|-------------|
| `format` | `g360-ventas-canonical` |
| `app`, `app_version` | Productor y versión |
| `schema_version` | `DIM_SCHEMA_VERSION` (semántica de dims/derivadas) |
| `exported_at` | Marca temporal UTC de la exportación |
| `db_file`, `db_size_bytes` | Archivo y tamaño |
| `total_rows_raw` | Filas totales (raw-first: **todas** las líneas) |
| `min_mes`, `max_mes`, `total_months` | Cobertura temporal |
| `checksums[]` | `{mes_ref, checksum, filas, soles}` — **baseline de validación** |
| `filters` | Política de la instantánea (raw-first; sin filtro de líneas ni retención) |

## Protocolo del consumidor

1. **Importar**: copiar el `.db` a la carpeta de la app consumidora y abrirlo como
   su base de trabajo (SQLite local). Aislado del productor.
2. **Validar** contra el manifiesto antes de usar:
   - comparar `total_rows_raw`, `total_months`, `min_mes`, `max_mes`;
   - comparar por mes `checksum`, `filas`, `soles` (tabla `mes_checksums`).
   - si algo no cuadra, no usar la copia y pedir nueva instantánea.
3. **Trabajar aislado**: crear las tablas/vistas de la app consumidora con **prefijo
   propio** en su copia; nunca escribir en el `.db` compartido.
4. **Actualizar**: re-importar una instantánea nueva cuando el productor capture
   datos nuevos (no hay sync bidireccional).

## Llaves canónicas (para joins)

| Entidad | Llave |
|---------|-------|
| Cliente | `id_cliente` o `doc_cliente` |
| Artículo/SKU | `id_articulo` u `original_sku` |
| Documento | `folio_unico` (= `tpo_doc/serie_doc/nro_doc`) |
| Factura ↔ NC/ND | `serie_doc, nro_doc` ↔ `factura_ref_serie, factura_ref_nro` |
| Período | `mes_ref`, `anho`, `mes`, `fecha_orig` |

Tablas de apoyo: `dim_cliente`, `dim_articulo`, `dim_documento`, `dim_vendedor`,
`dim_linea`, `fact_venta_mes`, y vistas `vw_dim_*`.

## Reglas

- `doc_cliente`: se conserva **una sola vez** (primera aparición del encabezado). El
  segundo `DOC_CLIENTE` del ERP se **ignora** (queda solo informativo si difiere).
- **Sin unicidad** sobre `folio_unico`: una factura puede tener varias líneas del
  mismo SKU (gratuita + pagada, guía de remisión). No imponer `unique`.
- `mes_ref` es la clave temporal (`YYYY-MM`); usar `mes_ref` y no el texto del ERP.
- Consumidor en **otra máquina**: no abrir el SQLite por red; usar la instantánea
  (o un derivado remoto curado, p. ej. Supabase).

## Cuándo pedir un campo nuevo

1. Si ya está en la instantánea → agregar la consulta/join documentada.
2. Si solo está en el derivado remoto → usar ese derivado.
3. Si no existe → decidir si se deriva, se extrae del ERP o no corresponde.
   No crear tablas paralelas ni duplicar el histórico.
