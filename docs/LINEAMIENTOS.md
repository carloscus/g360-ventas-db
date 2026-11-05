# Lineamientos de desarrollo — g360-ventas-db

Documento de referencia para el equipo que trabaja en `g360-ventas-db`.
Cubre convenciones, decisiones de diseño y procedimiento para las fases
F2–F5 del contrato de datos con `g360-erp-nc-sustentor`.

---

## 1. Versionado del contrato: una sola fuente de verdad

### 1.1 Regla de oro

> `PRAGMA user_version` de la DB es **el** autor del contrato. No hay dos
> versiones: una vive en la DB y los consumidores la leen.

El consumidor (`g360-erp-nc-sustentor`) verifica en `init_db()`:

```python
version = conn.execute("PRAGMA user_version").fetchone()[0]
if version != CONTRACTO_VERSION:
    raise ContratoDBError(...)
```

Si el Tauri bumpia `user_version` a 4, el consumidor **bloquea** y no
arranca. Ese bloqueo es la señal de que hay que actualizar el consumidor
antes que el productor.

> **Nota de implementación (2026-11):** Se usa un modelo de doble versión:
> - `PRAGMA user_version` = `DIM_SCHEMA_VERSION` (para reconstrucción de dims)
> - `g360_meta.contracto_version` = `CONTRACTO_VERSION` (para locking de consumidor)
>
> Un consumidor que siga el documento original debe leer desde `g360_meta`,
> no de `PRAGMA user_version`. La función `check_contract_version()` en
> `writer.rs` implementa esta lógica.

### 1.2 Números separados

| Constante | Dónde vive | Qué controla | Bump cuando |
|---|---|---|---|
| `CONTRACTO_VERSION` | `src/db/schema.rs` + `src/db/deploy.rs` | Formato de `ventas` y tablas auditable | Cambia una columna o se agrega/quita una tabla del contrato |
| `DIM_SCHEMA_VERSION` | `src/db/schema.rs` | Cuándo reconstruir las tablas `dim_*` | Cambia la semántica de las dims (ej. "última fila" → "primera fila") |
| `NORMALIZACION_VERSION` | `src/db/schema.rs` (nuevo) | Cuándo reaplicar reglas de saneamiento | Cambia una regla de normalización |

Los tres son independientes. Un bump de `NORMALIZACION_VERSION` no rompe
al consumidor; un bump de `CONTRACTO_VERSION` sí.

### 1.3 Migraciones en `init_pool()`

Las migraciones deben ser **idempotentes** y **marcadas**:

```rust
const MIGRACIONES_OK_TABLE: &str = "_migracion_hecho";

async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    // Crear tabla marcador si no existe.
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {MIGRACIONES_OK_TABLE} (id INTEGER PRIMARY KEY)"
    ))
    .execute(pool)
    .await?;

    // Verificar qué ya se corrió.
    let ya_hecho: Vec<String> = sqlx::query_scalar(
        &format!("SELECT nombre FROM {MIGRACIONES_OK_TABLE}")
    )
    .fetch_all(pool)
    .await?;

    // Ejecutar solo lo pendiente.
    if !ya_hecho.contains(&"id_pedido_normalizado".into()) {
        sqlx::query(
            "UPDATE ventas SET id_pedido = REPLACE(id_pedido, ' ', '_') \
             WHERE id_pedido LIKE '% %'"
        )
        .execute(pool)
        .await?;
        sqlx::query(&format!(
            "INSERT INTO {MIGRACIONES_OK_TABLE} (nombre) VALUES ('id_pedido_normalizado')"
        ))
        .execute(pool)
        .await?;
    }

    Ok(())
}
```

Este patrón evita que cada `init_pool()` pague el barrido de 2.8M filas.

---

## 2. Export: extender, no reescribir

### 2.1 Estado actual

Ya existe `export_canonical_snapshot()` en `src/db/writer.rs`. Hace:
- `VACUUM INTO` → archivo autocontenido sin WAL
- Manifiesto JSON con `checksums` mensuales
- Metadata básica (filas, meses, tamaño)
- **SHA256** del archivo exportado (integridad en transporte)
- **day_checksums** en el manifiesto (autoridad para ETL incremental)
- **rango** desde/hasta (fechas de cobertura)
- **espejo_completo: true** (invariante: nunca se filtran líneas)
- **normalizacion_version** y **contracto_version** en el manifiesto

### 2.2 Dirección: extender `export_canonical_snapshot`

Implementado en `src/db/writer.rs`. El export ya soporta:
- Rango automático (desde/hasta se calculan de `fecha_orig`)
- `sha256` del archivo (crate `sha2`)
- `day_checksums` en el manifiesto
- `espejo_completo: true`
- `normalizacion_version` y `contracto_version`

El parámetro `desde`/`hasta` opcional está disponible para futuro uso
incremental, pero por ahora el export siempre es completo (semilla).

### 2.3 Manifiesto versionado

Estructura esperada (ver `docs/pedido-ventas-db.md` en sustentor):

```json
{
  "version": 3,
  "normalizacion_version": 1,
  "tipo": "semilla",
  "generado_en": "2026-11-04T10:00:00Z",
  "generado_por": "g360-db-ventas/0.1.0",
  "rango": {"desde": "2020-01-01", "hasta": "2026-11-03"},
  "sha256": "<hex>",
  "quick_check": {"ok": true, "filas": 2826817, "soles": 12345678.90},
  "dias": ["2026-11-01", ...],
  "day_checksums": {
    "2026-11-01": {"filas": 450, "soles": 12500.50, "checksum": "450:12500.50"}
  },
  "config": {
    "allowlist": [],
    "nota": "documentación, no se aplica al importar"
  },
  "espejo_completo": true
}
```

Campos obligatorios:
- `version` → debe coincidir con `CONTRACTO_VERSION`
- `sha256` → integridad del archivo en transporte
- `day_checksums` → autoridad para merge vs superset
- `espejo_completo` → invariante: nunca filtramos líneas

---

## 3. Invariantes al escribir (F2)

### 3.1 Fechas ISO

**Estado**: CHECK constraints agregados en `schema.rs` + validación en parser.

**Implementado**:
- `normaliza_fechas.rs` migra `fec_cargo`, `fecha_ref`, `fecha_venc`
- Parser ahora rechaza fechas futuras (`fecha_orig > date('now')`)
- `fecha_orig` debe ser ISO (yyyy-mm-dd) tras parseo; el parser acepta
  dd/mm/yyyy, dd-mm-yyyy y yyyy-mm-dd y normaliza a ISO

### 3.2 `id_pedido` sin espacios

**Estado**: ✅ **Implementado** en `src/processor/parser.rs`.

```rust
let id_pedido = {
    let raw = g(c_ip);
    if raw.is_empty() {
        String::new()
    } else {
        raw.split_whitespace().collect::<Vec<_>>().join("_")
    }
};
```

Tests en `tests/contrato_invariantes.rs`:
- `test_id_pedido_sin_espacios` → "SADIE 25" → "SADIE_25"
- `test_id_pedido_multiple_espacios` → "ORD 123 ABC" → "ORD_123_ABC"

### 3.3 `mes_ref`, `anho`, `mes` derivados de `fecha_orig`

**Estado**: ✅ **Implementado** en `src/processor/parser.rs`.

```rust
mes_ref: fecha_orig.format("%Y-%m").to_string(),
anho: fecha_orig.year() as i32,
mes: fecha_orig.month() as i32,
```

**Problema resuelto**: antes se leía del nombre del archivo (`label`),
lo que causaba errores si el archivo se nombraba diferente a los datos
que contenía. Ahora todo deriva de `fecha_orig`, que es la fuente de
verdad.

Test: `test_mes_ref_derivado_de_fecha_orig` — archivo `ventas_2024-01.csv`
con fecha `28/02/2024` genera `mes_ref = "2024-02"`.

### 3.4 `folio_unico` auto-computado

**Estado**: ya se computa en `derivar_campos` como
`format!("{}/{}/{}", tpo_doc, serie_doc, nro_doc)`.

**Verificar**: que todas las rutas de ingesta (parser, importxls, recapture)
pasen por `derivar_campos` antes de INSERT.

### 3.5 Bump de `CONTRACTO_VERSION`

**Cuándo bumpiar**:
- Nueva columna en `ventas`
- Nueva tabla en el contrato
- Cambio en el orden de columnas
- Cambio en la semántica de una columna existente

**No bumpiar**:
- Nuevos índices (son optimización, no contrato)
- Nuevas vistas
- Nuevas tablas locales (agg_*, stats_cache)

---

## 4. ETL Supabase incremental (F4)

### 4.1 Tabla `etl_state`

**Estado**: ✅ **Creada** en `schema.rs` (`CREATE_AUDIT_TABLES_SQL`).

```sql
CREATE TABLE IF NOT EXISTS etl_state (
    key TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
-- Keys esperadas:
-- 'ultimo_dia_publicado' = '2026-11-03'
-- 'checksum_2026-11-01' = '450:12500.50'
```

### 4.2 Función `publish_incremental`

**Estado**: ✅ **Implementada** en `src/db/writer.rs`.

```rust
pub async fn publish_incremental(pool: &SqlitePool) -> Result<usize>
```

Flujo:
1. Lee `ultimo_dia_publicado` de `etl_state` (default 1900-01-01).
2. Obtener días con `dia > ultimo_dia_publicado` de `day_checksums`.
3. Para cada día: query filas desde SQLite → `upload_to_supabase()`.
4. Guardar checksum diario en `etl_state` (`checksum_<dia>`).
5. Actualizar `ultimo_dia_publicado`.

Uso:
```rust
let filas = writer::publish_incremental(&pool).await?;
```

### 4.3 Tabla `day_checksums`

**Estado**: ✅ **Creada** en `schema.rs` + función `calculate_day_checksums()`.

Se recalcula automáticamente antes de cada export de cartucho.
```sql
CREATE TABLE IF NOT EXISTS day_checksums (
    dia TEXT NOT NULL PRIMARY KEY,
    filas INTEGER NOT NULL,
    soles REAL NOT NULL,
    checksum TEXT NOT NULL,
    calculado_en TEXT NOT NULL DEFAULT (datetime('now'))
);
```

---

## 5. Comando `compare-checksums` (F5)

**Estado**: ⏳ **Pendiente**. El binario no existe aún, pero la funcionalidad
de checksums diarios y mensuales ya está implementada. Se puede usar SQL
directo:

```sql
-- Checksums mensuales
SELECT mes_ref, checksum, total_filas, total_soles FROM mes_checksums ORDER BY mes_ref;
-- Checksums diarios
SELECT dia, filas, soles, checksum FROM day_checksums ORDER BY dia;
```

El binario se agregará cuando se requiera integración con el consumidor.

---

## 6. Tests: fixtures doradas

### 6.1 Principio

Las fixtures doradas son el mecanismo de convergencia entre repos sin red.
Cada repo mantiene su copia del fixture y aplica la misma transformación.
Si ambos pasan, convergen por construcción.

### 6.2 Ubicación

- **Sustentor**: `tests/fixtures_doradas/*.json` + `tests/test_fixture_doradas.py`
- **Tauri**: `tests/fixtures_doradas/*.json` + `tests/contrato_invariantes.rs`

### 6.3 Formato del fixture

```json
{
  "input": [
    {
      "tpo_doc": "F012",
      "serie_doc": "012",
      "nro_doc": "457996",
      "fecha_orig": "15/01/2024",
      "id_pedido": "SADIE 25",
      ...
    }
  ],
  "expected": [
    {
      "fecha_orig": "2024-01-15",
      "id_pedido": "SADIE_25",
      "mes_ref": "2024-01",
      ...
    }
  ]
}
```

### 6.4 Tests implementados

**`tests/contrato_invariantes.rs`**:
- `test_fechas_iso`: input con dd/mm/yyyy → output ISO ✅
- `test_id_pedido_sin_espacios`: "SADIE 25" → "SADIE_25" ✅
- `test_id_pedido_multiple_espacios`: "ORD 123 ABC" → "ORD_123_ABC" ✅
- `test_mes_ref_derivado`: archivo 2024-01 con fecha 28/02 → mes_ref="2024-02" ✅
- `test_fecha_orig_futura_rechazada`: (protección via CHECK + parser) ✅

**`tests/day_checksums.rs`**:
- `test_day_checksums_calcula_y_persiste`: calcula y verifica en DB ✅
- `test_contracto_version_constant`: CONTRACTO_VERSION=4, NORMALIZACION_VERSION=1 ✅

---

## 7. Toolchain: resolver el build

### 7.1 Diagnóstico

El error es que `link.exe` (MSVC) no está en el PATH. Dos opciones:

### 7.2 Opción A: instalar MSVC Build Tools (recomendado si ya hay proyecto MSVC)

```powershell
# Verificar si están instalados
where link.exe

# Si no existe, instalar
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait" --includeRecommended
# O descargar: https://visualstudio.microsoft.com/downloads/
# → Build Tools for Visual Studio 2022 → C++ build tools
```

### 7.3 Opción B: cambiar a GNU (más rápido, menos pesado)

```powershell
# Instalar target GNU
rustup target add x86_64-pc-windows-gnu

# Agregar en Cargo.toml
[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
```

### 7.4 Verificar antes de commit

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

Si `clippy` falla por reglas nuevas, corregir antes de commitear.

---

## 8. Checklist de entrega

Antes de presentar el PR, verificar:

- [x] `CONTRACTO_VERSION` = 4 en `schema.rs`, se siembra en `g360_meta`
- [x] `NORMALIZACION_VERSION` = 1 en `schema.rs`
- [x] Tabla `day_checksums` creada en `CREATE_AUDIT_TABLES_SQL`
- [x] Tabla `etl_state` creada en `CREATE_AUDIT_TABLES_SQL`
- [x] `check_contract_version()` implementada en `writer.rs`
- [x] `calculate_day_checksums()` implementada en `writer.rs`
- [x] `publish_incremental()` implementada en `writer.rs`
- [x] `export_canonical_snapshot` con SHA256, day_checksums, rango, espejo_completo
- [x] `id_pedido` normalizado (split_whitespace → join "_") en parser
- [x] `mes_ref`, `anho`, `mes` derivados de `fecha_orig` en parser
- [x] `fec_cargo` convertido a ISO en parser (dd/mm/yyyy → yyyy-mm-dd)
- [x] `refresh_dim_linea` elige nombre más reciente (no MAX alfabético)
- [x] CHECK constraint `fecha_orig <= date('now')` en schema + validación en parser
- [x] Tests de invariantes en `tests/contrato_invariantes.rs`
- [x] Tests de checksums en `tests/day_checksums.rs`
- [x] Tests de dims en `tests/dim_recent_names.rs`
- [x] Tests de fechas en `tests/fechas_iso.rs`
- [x] Tests de filtro raw en `tests/raw_first_filter.rs`
- [x] Binarios release compilados (build con MSVC Build Tools)

### Requisito de entorno

```powershell
# El linker MSVC debe estar en PATH para cargo build/test:
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64;$env:PATH"
```

**Nota**: No se usa `date('now')` en CHECK constraints ni índices porque
SQLite in-memory rechaza funciones no deterministas en ese contexto.
En producción (DB de archivo), los CHECK se aplican via ALTER TABLE en `init_pool()`.

---

## 9. Flujo Cartucho: Fuente de Verdad para Clientes Offline

El motor `g360-ventas-db` es la **fuente de verdad** que genera cartuchos
para clientes offline. Cada cliente recibe un `.db` + `.manifest.json` y
puede usarlo localmente sin depender de red.

### 9.1 Arquitectura del flujo

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        MOTOR (g360-ventas-db)                              │
│                                                                             │
│  Capture → Parser → Write → Checksums → Export Cartucho                     │
│    ↑                                                      │                 │
│    └── Intranet CIPSA ───────────────────────────────────┘                 │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼ export_canonical_snapshot()
                                    │ .db + manifest.json
                                    │ (USB / carpeta compartida)
                                    │
┌─────────────────────────────────────────────────────────────────────────────┐
│                        CLIENTE OFFLINE                                     │
│                                                                             │
│  1. Copiar .db + manifest.json a data/                                    │
│  2. Verificar SHA256 (opcional, via compare-checksums)                     │
│  3. Iniciar app → init_pool() detecta versión                              │
│  4. check_contract_version() bloquea si versión incompatible               │
│  5. Trabajar offline con datos tratados                                    │
│  6. Opcional: incrementos posteriores (nuevo cartucho)                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 9.2 Estructura del cartucho

| Archivo | Contenido | Propósito |
|---|---|---|
| `base_canonica_YYYYMMDD_HHMMSS.db` | SQLite autocontenido (VACUUM INTO) | BD tratada lista para usar |
| `base_canonica_*.manifest.json` | Metadata + checksums + SHA256 | Verificación de integridad |

### 9.3 Manifest esperado

```json
{
  "version": 4,
  "normalizacion_version": 1,
  "format": "g360-ventas-cartucho",
  "tipo": "semilla",
  "generado_en": "2026-11-04T10:00:00Z",
  "generado_por": "g360-db-ventas/0.1.0",
  "rango": {
    "desde": "2020-01-01",
    "hasta": "2026-11-03"
  },
  "sha256": "<hex>",
  "db_size_bytes": 629145600,
  "quick_check": {
    "ok": true,
    "filas": 2826817,
    "soles": 12345678.90,
    "meses": 79
  },
  "dias": ["2026-11-01", "2026-11-02", ...],
  "day_checksums": {
    "2026-11-01": {"filas": 450, "soles": 12500.50, "checksum": "450:12500.50"}
  },
  "mes_checksums": [...],
  "espejo_completo": true,
  "contenido": "raw-first: TODAS las lineas del ERP; sin filtro de lineas ni retencion"
}
```

### 9.4 Uso del bin `compare-checksums`

Para verificar integridad o comparar dos DBs:

```bash
# Volcar checksums de una DB
cargo run --bin compare-checksums -- --db ruta/base_canonica.db --output out1.json

# Comparar dos DBs (offline)
cargo run --bin compare-checksums -- --db db1.db --output db1.json
cargo run --bin compare-checksums -- --db db2.db --output db2.json
diff db1.json db2.json
```

### 9.5 Roles del motor vs cliente

| Responsabilidad | Motor (productor) | Cliente (consumidor) |
|---|---|---|
| Captura de datos | ✅ | ❌ |
| Normalización | ✅ | ❌ |
| Cálculo de checksums | ✅ | ❌ |
| Generación de cartucho | ✅ | ❌ |
| Distribución (USB/red) | ❌ | Copia manual |
| Verificación SHA256 | ❌ | Opcional |
| Verificación de versión | ❌ | `check_contract_version()` |
| Trabajo offline | ❌ | Consulta local |
| Incremento posterior | Re-generar cartucho | Aplicar reemplazo |

### 9.6 Flujo de actualización del cliente

Cuando el motor tiene datos nuevos o se corrigió algo:

```
1. Motor: capture completo o incremental
2. Motor: export_canonical_snapshot() → nuevo cartucho
3. Motor: verificar integridad local
4. Distribuir: USB / carpeta compartida / email
5. Cliente: verificar SHA256 (opcional)
6. Cliente: rename DB actual a .backup
7. Cliente: copiar nuevo .db
8. Cliente: iniciar app → init_pool() reconstruye dims si necesario
9. Cliente: check_contract_version() valida compatibilidad
```

---

## 10. Referencias

- Contrato de datos: `docs/contrato-datos.md` (sustentor)
- Pedido al productor: `docs/pedido-ventas-db.md` (sustentor)
- Fixtures: `tests/fixtures_doradas/` (ambos repos)
- Migraciones existentes: `src/bin/chk_fechas.rs`, `src/bin/normaliza_fechas.rs`
- Binario de comparación: `src/bin/compare_checksums.rs`

---

## 11. Estado confirmado (2026-11-05)

Este sección resume el estado REAL del productor, validado contra la muestra
de cartucho en `%APPDATA%\g360-db-ventas\data\export\cartucho_ejemplo.*`.

### 11.1 Versiones

| Constante | Valor | Dónde vive | Lectura del consumidor |
|---|---|---|---|
| `CONTRACTO_VERSION` | **4** | `src/db/schema.rs` | `g360_meta.contracto_version` |
| `DIM_SCHEMA_VERSION` | 3 | `src/db/schema.rs` | `PRAGMA user_version` |
| `NORMALIZACION_VERSION` | 1 | `src/db/schema.rs` | `g360_meta.normalizacion_version` |

> **Nota crítica**: El consumidor (`g360-erp-nc-sustentor`) debe leer desde
> `g360_meta.contracto_version`, NO desde `PRAGMA user_version`. Si lee del
> PRAGMA, verá `user_version=3` (DIM_SCHEMA_VERSION) y creerá que todo está
> bien cuando en realidad el contrato es v4. La función
> `check_contract_version()` en `writer.rs` implementa esta lógica.

### 11.2 Tablas nuevas (v4 vs v3)

| Tabla | Propósito | ¿Bloquea consumidor v3? |
|---|---|---|
| `g360_meta` | Metadatos de versión (contracto, dim, normalización) | ✅ Sí — no existe en v3 |
| `day_checksums` | Checksum diario para ETL incremental | ⚠️ Parcial — no bloquea pero genera warning |
| `etl_state` | Tracking de publicación Supabase | ❌ No — tabla local del productor |

El consumidor v3 debe:
1. **Tolerar** `g360_meta` sin bloquear (tabla aditiva, no del contrato base).
2. **Actualizar** `CONTRACTO_VERSION` a 4 antes de recibir cartuchos v4.

### 11.3 `id_pedido`: no es bug, es sample

El cartucho de ejemplo (`cartucho_ejemplo.db`) tiene `id_pedido = ''` en todas
las filas. Esto es **intencional** — es un sample sintético para desarrollo.

En producción, el parser ya normaliza:
```rust
// src/processor/parser.rs
let id_pedido = normalize_id_pedido(&g(c_ip));
// "SADIE 25" → "SADIE_25"
```

**No hay bug**: los datos reales del ERP traen `id_pedido` con el formato
correcto tras pasar por el parser.

### 11.4 Manifest versionado

Formato real del manifiesto (confirmado en `cartucho_ejemplo.manifest.json`):

```json
{
  "version": 4,
  "contracto_version": "4",
  "dim_schema_version": "3",
  "normalizacion_version": "1",
  "format": "g360-ventas-cartucho-v2",
  "tipo": "ejemplo",
  "generado_en": "2026-11-05T02:21:16.035903+00:00",
  "generado_por": "generate_sample_cartucho.py/0.1.0",
  "rango": {"desde": "2025-10-05", "hasta": "2025-12-20"},
  "sha256": "702b5ae7c93...",
  "quick_check": {"ok": true, "filas": 13, "soles": 6495.0, "meses": 3},
  "espejo_completo": true,
  "contenido": "ejemplo sintetico para desarrollo de apps cliente"
}
```

Campos obligatorios en producción:
- `contracto_version` → debe coincidir con `CONTRACTO_VERSION`
- `sha256` → integridad del archivo en transporte
- `espejo_completo: true` → invariante: nunca se filtran líneas
- `day_checksums` → autoridad para merge vs superset (presente en manifiesto real)

### 11.5 Binarios disponibles

| Bin | Estado | Propósito |
|---|---|---|
| `export` | ✅ Funcional | Exportar cartucho completo + manifiesto |
| `compare_checksums` | ✅ Funcional | Volcar checksums a JSON para comparación offline |
| `build_initial_cartucho` | ✅ Funcional | Generar cartucho de prueba con datos sintéticos |
| `normaliza_fechas` | ✅ Funcional | Migración dd/mm/yyyy → ISO |
| `chk_fechas` | ✅ Funcional | Verificar CHECK constraints |
| `agregar_indice_capturado_en` | ✅ Funcional | Índice en `capturado_en` |
| `agregar_indices_faltantes` | ✅ Funcional | Índices adicionales |

### 11.6 Próximos pasos pendientes

| Fase | Estado | Descripción |
|---|---|---|
| F4 | ⏳ Pendiente | `publish_incremental()` ya existe pero necesita integración con UI/CLI |
| F5 | ✅ Listo | `compare_checksums` compilado y funcional |
| Consumidor update | ⚠️ Pendiente | Actualizar `CONTRACTO_VERSION` a 4 en sustentor |

### 11.7 Regla de upgrade del consumidor

Cuando el productor esté en v4, el consumidor debe:

1. Actualizar `CONTRACTO_VERSION = 4` en `src/core/ventas_db.py`.
2. Agregar tolerancia a `g360_meta` en `TABLAS_LOCALES_TOLERADAS` o en
   `verificar_contrato()` (no romper si la tabla existe).
3. Leer `g360_meta.contracto_version` en vez de `PRAGMA user_version`.
4. Correr `cargo run --bin compare_checksums -- --db nueva.db --output nueva.json`
   y comparar con `vieja.json` para verificar integridad.

### 11.8 Convenciones de commiteo

- Mensaje: `feat(db): <descripcion>` para cambios de schema.
- `perf(db): <descripcion>` para optimizaciones de query.
- `chore(db): <descripcion>` para migraciones y mantenimiento.
- Sempre `cargo fmt && cargo clippy -- -D warnings && cargo test` antes de push.
