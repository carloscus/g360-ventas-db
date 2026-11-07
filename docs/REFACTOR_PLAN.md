# Plan de Refactor — división de módulos grandes

> Auditoría de arquitectura (nov-2026) → **EJECUTADO en su mayor parte**
> (pasos 1-5, commits `d34a207`…`7f16491`). Tests: **26 suites verdes en
> cada paso**. Este documento queda como registro de lo hecho y de lo
> pendiente menor.

## Estado final

| Paso | Qué | Commit | Estado |
|---|---|---|---|
| 1 | `main.rs` → `commands/`×8 + `state.rs` + `types.rs` | `d34a207` | ✅ Hecho |
| 2 | `uploader.rs` → facade + `purge`/`reconcile`/`sync`/`info` | `5a063a7` | ✅ Hecho |
| 3 | Dedup buffer −1 día → `db::freshness` + **fix `sync_from_last`** | `6faa298` | ✅ Hecho (parcial ver detalle) |
| 4 | `capture.rs`: ruta secuencial delega en `process_month` | `51e184e` | ✅ Hecho (diferente al plan original) |
| 5 | A4: comandos sin UI → eliminados por lotes | `7f16491` | ✅ Hecho (opción a) |
| 6 | `[workspace]` raíz (un solo Cargo.lock) | — | ⏸️ No hecho (opcional) |

### Resultado medido

| Métrica | Antes | Después |
|---|---|---|
| `src-tauri/src/main.rs` | 2 475 líneas / 62 comandos | **~215 líneas / 26 comandos** |
| `src/processor/uploader.rs` | 1 773 líneas | **87 líneas (fachada)** + 4 módulos |
| `src/capture.rs` | 1 433 líneas | **1 075 líneas** |
| Comandos registrados | 62 (39 sin UI) | **26** (25 usados + 1 conservado) |

---

## Detalle por paso (qué se hizo vs. qué se planificó)

### 1. `main.rs` → `commands/` ✅

- Split **mecanizado con script** que valida **cobertura 2475/2475 líneas**
  (ni una perdida) ni solapes antes de escribir.
- Nueva estructura: `state.rs` (statics/caches `pub(crate)`), `types.rs`
  (structs de respuesta), `commands/` con 8 dominios
  (dashboard, config, capture, sync, months, audit, cartucho, maintenance).
- `commands/mod.rs` centraliza `verify_intranet_credentials` (compartido).
- Lecciones (por si se repite): `#[tauri::command]` solo genera
  `#[macro_export]` si la fn es `pub` → 53 fns hechas `pub`;
  `#[macro_use] mod commands;` para que `generate_handler!` resuelva los
  macros `__cmd__*`; `use state::CAPTURE_STATE;` en `main`.

### 2. `uploader.rs` → 4 módulos ✅

- `purge.rs` (purga por lotes) · `reconcile.rs` (mes por mes + circuit
  breaker) · `sync.rs` (upload/incremental/dry-run) · `info.rs` (consultas
  ligeras de Supabase).
- Fachada `uploader.rs` conserva `VENTA_SELECT_COLS` + `map_venta_row`
  (compartidos; visibles a los hijos por regla de ancidencia).
- Rutas públicas intactas vía `pub use` (`uploader::ProgressCb`,
  `uploader::reconcile_supabase`, …).

### 3. Dedup buffer −1 día ✅ (parcial)

- **Hecho**: `src/db/freshness.rs` con `last_captured_date(pool, buffer_days)`
  + `CAPTURE_BUFFER_DAYS = 1`; consumido por `sync_smart`,
  `sync_from_last` y `run_auto_sync`. **Fix real**: `sync_from_last` no
  aplicaba buffer (perdía documentos tardíos) — ahora las 3 rutas usan el
  mismo criterio. Algoritmo civil-date manual (era/doe) eliminado.
- **No hecho (menor)**: `parse_date` (dd/mm/yyyy) sigue duplicado entre
  `parser.rs` (privado) y `bin/normaliza_fechas.rs`; SHA-256 de manifest
  tiene 2 copias (`writer.rs` vs `bin/build_initial_cartucho.rs`).

### 4. `capture.rs` — ejecutado con un enfoque DISTINTO al plan ✅

El plan original decía "extraer `download_month_dayloop` ×4". El análisis
previo (diff semántico) mostró el problema real:

- `process_month` (~410 líneas) y `run_batch_history` compartían **92.2 %**
  de su lógica: la ruta **secuencial** (`num_workers == 1`) tenía el cuerpo
  del mes **inline** (≈390 líneas) mientras la ruta **concurrente** ya
  delegaba en `process_month`.
- Divergencias reales mapeadas antes de tocar: refresh de sesión c/6 meses
  (ambas rutas), `sys_fail` circuit breaker + `fail.push` en systemic
  (`process_month` retorna systemic **sin** push), ETA+pacing (ya dentro de
  `process_month`), `session` vs `transport` (mismo tipo: `create_transport`).

**Solución aplicada** (`51e184e`): la ruta secuencial ahora llama a
`process_month(...)` y conserva en el caller solo el manejo de `sys_fail`
(incl. `fail.push`) + refresh c/6 + abort pre-check. `capture.rs`:
**1433 → 1075 líneas (−358)** y las 2 rutas comparten el mismo pipeline
de cero deriva futura.

- **No hecho (opcional)**: dividir `capture.rs` en `pipeline/month/lock`
  (con la duplicación eliminada, el valor baja y el riesgo no compensa).

### 5. A4 — comandos sin UI ✅ (opción a: eliminar, por lotes)

- Inventario exacto con fuente de verdad (`invoke('...')` vs
  `generate_handler`): **62 registrados / 23 usados / 39 sin UI**.
- **36 eliminados** en 3 lotes con `cargo check` + tests entre lote
  (script con validación: fn encontrado + salido del handler; aborta sin
  escribir si algo falla).
  - Lote 1: `sync_from_last`, `run_full_flow`, `capture_range`, `upload_all`,
    `reconcile_supabase_cmd`, `reset_sync_marker`, `upload_dry_run`,
    `upload_all_with_verify`, `calculate_checksums`, `recalculate_checksums`,
    `auto_daily_pipeline`, `get_next_capture_time`
  - Lote 2: `list_months`, `delete_month(s)`, `get_completeness`,
    `get_failed_days`, `import_manual_*`, `preview_import`, `preview_csv`,
    `reparse_raw`, `get_credentials_status`, `test_intranet`
  - Lote 3: `get_dashboard`, `get_local_metrics`, `get_db_health`,
    `clear_cache`, `clear_db`, `admin_reset`, `vacuum_db`, `export_full_db`,
    `verify_integrity`, `get/checksum_history`, `get_sync_history`,
    `get_contract_metadata`
- **3 conservados**: `abort_capture` (+botón ⏹ en UI), `save_service_role_key`
  (+cableado: el campo de Config existía pero nunca se guardaba — bug),
  `normalize_query_id_cmd` (entregado; UI de búsqueda pendiente).
- **Estado final: 26 registrados / 25 usados por la UI / 1 pendiente.**

### 6. `[workspace]` raíz ⏸️ No hecho

Ahorra un Cargo.lock doble y builds más rápidos. Bajo riesgo pero requiere
mover/verificar `Cargo.lock` y perfiles; dejar para sesión dedicada.

---

## Pendientes menores (bajo riesgo, sin urgencia)

1. `parse_date` → `pub` en `parser.rs` y reusar en `bin/normaliza_fechas.rs`
2. Helper único de SHA-256 de manifest (`writer.rs` ↔ `bin/build_initial_cartucho.rs`)
3. UI de búsqueda usando `normalize_query_id_cmd` (comando conservado)
4. Opcional: `[workspace]` raíz (paso 6)
5. Opcional: dividir `capture.rs` en submódulos (solo si vuelve a crecer)
6. **Logger en modo `--task-scheduler`**: `main()` solo inicializa `app.log`
   en la rama GUI (`main.rs` L135-160); la rama task (L128-132) corre sin
   subscriber y `run_auto_sync_and_exit` usa `println!` → Task Scheduler
   descarta el stdout. **Las corridas diarias (00:00/14:00) no dejan rastro
   auditable**. Fix: extraer `init_file_logging()` y llamarla en ambas ramas
   + rebuild + reinstalar (el task apunta al exe instalado en
   `AppData\Local\G360 DB Ventas\`). Detectado 06/11 durante la verificación
   del cron. → **EN CURSO 06/11** (fix aplicado, pendiente verificación).
7. **Dashboard hace `COUNT(*)` en vivo cada 2 min** (3-4.5 s sobre 2.8M filas,
   WARNs en `app.log`) en vez de leer `stats_cache` (que ya existe y se
   refresca post-captura). Ver observación de rendimiento 06/11.
8. **Cron: rama "solo captura local" inalcanzable** — `run_auto_sync_and_exit`
   sale con `exit(1)` si Supabase no está configurado (L20) *antes* de la
   rama que maneja "solo local" (L75). Una PC sin credenciales Supabase
   nunca podría correr el cron en modo local.
9. **Rotación de cartuchos no borra el `.zip` hermano** (writer.rs
   `rotate_*` solo elimina `.db`): cada zip huérfano = 338 MB en `export/`.
10. **7 warnings dead code del A4** (comandos borrados dejaron fns/types sin
    uso): `verify_intranet_credentials`, `reject_numeric_sku`,
    `DASHBOARD_CACHE_TTL_SECS`, `set_state`, `CompletenessInfo`,
    `FailedDayInfo`, `PreviewResult`.
11. **`PRAGMA optimize` (ANALYZE) en init**: cuesta ~71 s cuando los stats
    están obsoletos (p.ej. tras cambiar índices o después de capturas). En
    régimen normal no corre (medido 06/11: init <1 s), pero puede aparecer
    ~71 s en el primer arranque posterior a escrituras. Si molesta, gatearlo
    por `g360_meta` como el bloque de reparación.
12. **Deuda `cargo fmt`**: `src/db/writer.rs` tiene ~68 hunks preexistentes
    (nunca se formateó el archivo completo; formatearlo metería ruido ajeno
    en un commit funcional → hacerlo como commit `chore` aparte).
    `src-tauri/src/main.rs` y `src/processor/parser.rs` HEAD estaban limpios
    → se formateó solo su región editada (06/11).
13. **`refresh_dim_*` sin transacción + rebuild desproporcionado** (incidente
    06/11): cada `refresh_dim_x` hace `DELETE` + `INSERT..SELECT` **sin
    BEGIN/COMMIT** — un kill entre ambos deja la dim **vacía** (le pasó a
    `dim_documento`: 0 filas / 694K folios huérfanos tras cortar un proceso
    a los 420 s). Además, cambiar 6 filas disparó el rebuild **completo** de
    las 6 dims (~8 min). Fixes: (a) envolver cada refresh en transacción,
    (b) refresh selectivo por dim afectada, (c) `ensure_dims_fresh` revisa
    solo `dim_cliente` → ampliar la señal de "necesita rebuild" (dims
    huérfanas no se autocuran hoy).

## Protocolo seguido (para futuros refactors)

1. **Script de split con validación previa**: cobertura de líneas
   (sum(gaps)+sum(items) == total), sin solapes, sin items perdidos →
   aborta sin escribir si falla.
2. **Verde obligatorio por paso**: `cargo check --all-targets` + `cargo test`
   (26 suites) antes de cada commit checkpoint.
3. **`cargo fix --allow-dirty`** tras cada split (imports huérfanos).
4. **Commits incrementales** (`d34a207` → `5a063a7` → `6faa298` →
   `51e184e` → `7f16491`), todos pusheados: revertible por separado.
5. **Análisis semántico previo** en los no-mecánicos (paso 4: diff mostró
   divergencias reales → se mapearon y conservaron antes de tocar).
