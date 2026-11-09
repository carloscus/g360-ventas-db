# LINEAMIENTOS — g360-ventas-db

> Documentación de operación, contrato de datos y flujos ETL. Actualizado 2026-11-04.

---

## Tabla de Contenidos

- [Flujo ETL incremental (F4)](#flujo-etl-incremental-f4)
- [Contrato de datos v4](#contrato-de-datos-v4)
- [Checksums y auditoría](#checksums-y-auditoria)
- [Base canónica para consumidores offline](#base-canonica-para-consumidores-offline)
- [Retención y ventana Supabase](#retención-y-ventana-supabase)
- [Comandos CLI útiles](#comandos-cli-útiles)

---

## Flujo ETL incremental (F4)

### ¿Qué es?

`publish_incremental` es el mecanismo de publicación **día a día** hacia Supabase.
A diferencia del sync mensual (`reconcile_supabase`), este flujo:

1. Lee el último día publicado desde `etl_state`.
2. Calcula checksums diarios (`day_checksums`) con el timestamp actual.
3. Para cada día desde `ultimo + 1` hasta hoy:
   - Compara el checksum local con el publicado (`checksum_<dia>` en `etl_state`).
   - Si **no cambió** → salta el día.
   - Si **cambió** → purga + re-sube las filas de ese día en Supabase.
4. Actualiza `etl_state` con el último día publicado.

### Binario `publish`

```bash
# Publicar días nuevos/modificados
cargo run --bin publish

# Forzar publicación completa (resetea estado incremental)
cargo run --bin publish -- --full
```

### Salida del comando

JSON con métricas de la ejecución:

```json
{
  "dias_escaneados": 5,
  "dias_con_cambios": 2,
  "dias_saltados": 3,
  "filas_subidas": 1250,
  "filas_purgadas": 0,
  "ultimo_dia_publicado": "2026-11-04",
  "desde": "2026-01-10",
  "hasta": "2026-11-04",
  "publicado_en": "2026-11-04T12:00:00Z"
}
```

### ¿Cuándo usar `--full`?

- Después de una migración de esquema que afecte datos históricos.
- Cuando se detectó drift entre local y Supabase.
- Para reconstruir la ventana completa desde cero.

⚠️ **Precaución**: `--full` borra el estado incremental y fuerza una re-publicación
completa. En BD grandes puede tomar varios minutos.

---

## Contrato de datos v4

| Versión | Cambio |
|---------|--------|
| `v1` | Esquema inicial de `ventas` |
| `v2` | Agregado `tipo_operacion`, `folio_unico` |
| `v3` | Agregado `factura_ref_serie`, `factura_ref_nro` |
| `v4` | Agregado `day_checksums`, `etl_state`, tabla `g360_meta`; CHECK constraints en fechas ISO |

Los consumidores leen `g360_meta.contracto_version` para decidir si bloquean.
El valor se guarda en PRAGMA `user_version` para dims (`DIM_SCHEMA_VERSION = 3`).

---

## Checksums y auditoría

### Checksum mensual (`mes_checksums`)

Key: `printf('%08x-%08x-%08x-%08x', COUNT(*), SUM(soles)*100, SUM(dolares)*100, SUM(cantidad)*100)`

Se usa en `reconcile_supabase` para detectar meses que cambiaron.

### Checksum diario (`day_checksums`)

Key: `COUNT(*) || ':' || ROUND(COALESCE(SUM(soles), 0), 2)`

Se usa en `publish_incremental` para detectar días con cambios.
El timestamp de cálculo se pasa como parámetro para mantener determinismo en tests.

### Estado ETL (`etl_state`)

| Key | Valor |
|-----|-------|
| `ultimo_dia_publicado` | `'2026-11-04'` |
| `checksum_YYYY-MM-DD` | `'123:4567.89'` |

---

## Base canónica para consumidores offline

Ver [`docs/base-canonica-consumo.md`](docs/base-canonica-consumo.md) para el protocolo completo.

Resumen rápido:

```bash
# Generar snapshot canonico
cargo run --bin export

# Output: base_canonica_<ts>.db + base_canonica_<ts>.manifest.json
# El consumidor valida contra el manifiesto antes de usar.
```

---

## Retención y ventana Supabase

| Configuración | Default | Descripción |
|---------------|---------|-------------|
| `supabase_retention_days` | 0 (usar años) | Ventana rotativa en días |
| `supabase_retention_years` | 4 | Ventana en años (fallback) |
| `app_retention_years` | 3 | Ventana para apps derivadas |

La BD local **nunca** hace purge — conserva historial completo desde 2010.

---

## Comandos CLI útiles

```bash
# Captura desde intranet
cargo run --bin recapture -- 2026-01 2026-11

# Re-parsear CSVs sin intranet
cargo run --bin reparse

# Importar XLS manual
cargo run --bin importxls -- 2026-10 /ruta/archivo.xls

# Consultar BD local
cargo run --bin query

# Reconciliación mensual (mes por mes)
cargo run --bin upload

# Publicación incremental (día a día)
cargo run --bin publish

# Exportar base canónica
cargo run --bin export

# Comparar checksums entre dos DBs
cargo run --bin compare-checksums -- --db ruta/db.db --output out.json

# Generar cartucho inicial desde cero
cargo run --bin build-initial-cartucho -- --db ./data/ventas.db --output ./exports/base
```

---

**G360 by ccusi** · Microherramientas para apoyo CRM y datos en CIPSA
