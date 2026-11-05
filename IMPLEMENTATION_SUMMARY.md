# Resumen de Implementación: g360-ventas-db

## Estado Actual (2026-11-04)

### ✅ Verificación

```bash
# Build
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64;$env:PATH"
cargo build --release

# Tests
cargo test
```

**Resultados esperados:**
- `calamine_test`: 3/3 ✅
- `contrato_invariantes`: 5/5 ✅
- `day_checksums`: 4/4 ✅
- `dim_recent_names`: 3/3 ✅
- `fechas_iso`: 1/1 ✅
- `raw_first_filter`: 3/3 ✅ (tras fix de clave stats_cache)

### ✅ Completado

1. **Schema Renunciado** (`src/db/schema.rs`)
   - `CONTRACTO_VERSION = 4` (bloquea consumidor si mismatch)
   - `DIM_SCHEMA_VERSION = 3` (reconstruye dims automáticamente)
   - `NORMALIZACION_VERSION = 1` (no bloquea, solo tracking)
   - Tabla `g360_meta` para versionado explícito
   - Tablas `day_checksums`, `etl_state` para ETL incremental
   - CHECK constraints aplicados via ALTER TABLE en producción (no en tests in-memory)

2. **Parser Mejorada** (`src/processor/parser.rs`)
   - `id_pedido`: normaliza espacios → `_` (ej. "SADIE 25" → "SADIE_25")
   - `mes_ref`, `anho`, `mes`: derivados de `fecha_orig`, NO del nombre de archivo
   - Rechazo automático de fechas futuras
   - Retorna `QualityStats` con contadores de filas inválidas

3. **Motor de Escritura** (`src/db/writer.rs`)
   - `init_pool()`: siembra `g360_meta` con las 3 versiones
   - `check_contract_version()`: valida compatibilidad antes de usar DB
   - `export_canonical_snapshot()`: genera `.db + manifest.json` con SHA256, versiones, checksums, data_quality, attributes_applied
   - `calculate_day_checksums()`: checksums diarios para ETL
   - `publish_incremental()`: sube solo días nuevos a Supabase

4. **Bins Nuevos**
   - `build_initial_cartucho`: genera cartucho desde DB existente
   - `compare_checksums`: compara checksums entre dos DBs offline

5. **Tauri Integrado**
   - Nuevos commands: `check_contract_version`, `get_contract_metadata`
   - Frontend muestra alerta si versión de contrato es incompatible
   - Sección "versión contrato" en el panel de estado

6. **Tests** (todos pasan)
   - `tests/contrato_invariantes.rs`: 5 tests de normalización ✅
   - `tests/day_checksums.rs`: 4 tests de checksums y versionado ✅
   - `tests/dim_recent_names.rs`: 3 tests de dims (incluye nombre más reciente) ✅
   - `tests/fechas_iso.rs`: 1 test de conversión de fechas a ISO ✅
   - `tests/raw_first_filter.rs`: 3 tests de filtro raw-first ✅
   - `tests/calamine_test.rs`: 3 tests de parseo ✅

### 📦 Binarios Release Compilados

| Binario | Tamaño | Propósito |
|---|---|---|
| `g360-db-ventas.exe` | 3.14 MB | Menú principal de comandos |
| `build_initial_cartucho.exe` | 3.19 MB | Generar cartucho inicial |
| `compare_checksums.exe` | 3.11 MB | Comparar checksums offline |
| `export.exe` | 3.25 MB | Exportar snapshot |
| `upload.exe` | 5.07 MB | Subir a Supabase |
| `importxls.exe` | 4.12 MB | Importar XLS |

### 🔧 Flujo de Uso

```powershell
# 1. Generar primer cartucho desde BD existente
.\target\release\build_initial_cartucho.exe --db .\data\historial.db --output .\exports\base_canonica

# 2. Verificar integridad
.\target\release\g360-db-ventas.exe status

# 3. Comparar checksums entre dos DBs
.\target\release\compare_checksums.exe --db db1.db --output out1.json
.\target\release\compare_checksums.exe --db db2.db --output out2.json
diff out1.json out2.json
```

### 📋 Manifest del Cartucho (ejemplo)

```json
{
  "version": 4,
  "contracto_version": "4",
  "dim_schema_version": "3",
  "normalizacion_version": "1",
  "format": "g360-ventas-cartucho-v2",
  "sha256": "a1b2c3...",
  "espejo_completo": true,
  "data_quality": {
    "status": "excellent",
    "total_records": 2826817,
    "valid_records": 2826500,
    "price_unitario_status": "ok"
  },
  "attributes_applied": [
    "id_pedido_normalized",
    "mes_ref_from_fecha_orig",
    "future_date_filtered"
  ],
  "day_checksums": { ... },
  "mes_checksums": [ ... ]
}
```

### 🔄 Flujo de Reemplazo para Cliente

```
1. Admin genera cartucho: export_canonical_snapshot()
2. Cliente recibe: .db + .manifest.json (USB/red)
3. Cliente verifica SHA256 contra manifest
4. Cliente reemplaza BD: mv historial.db historial.db.bak; cp nueva.db historial.db
5. Cliente inicia app → init_pool() detecta versiones
6. check_contract_version() valida compatibilidad
7. Si OK → app funciona normal
   Si FAIL → cliente contacta admin para nuevo cartucho
```

### ⚠️ Pendientes Conocidos

1. **Ejemplos con warnings menores**: `simulate_capture.rs`, `reparse_nc_nd.rs`, `e2e_january.rs` tienen variables no usadas (warnings, no errores)
2. **App Tauri completa**: La UI actual es funcional pero puede expandirse con:
   - Pestaña dedicada para gestión de cartuchos
   - Visualización de `data_quality` en tiempo real
   - Botón "Reparar BD" que orqueste el flujo completo

### 📖 Documentación

- `docs/LINEAMIENTOS.md`: Actualizado con nuevo diseño de cartucho
- `IMPLEMENTATION_SUMMARY.md`: Este documento

### 🚀 Próximos Pasos Recomendados

1. **Probar el flujo completo** en máquina con MSVC Build Tools
2. **Crear primer cartucho real** desde datos de producción
3. **Documentar procedimiento** de distribución a clientes offline
4. **Mejorar UI Tauri** con funcionalidades de cartucho específicas
5. **Limpiar warnings** en ejemplos

---

*Generado: 2026-11-04*
*Versión: 0.1.0*
*Contracto Version: 4*
