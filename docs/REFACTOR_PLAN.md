# Plan de Refactor — división de módulos grandes

> Auditoría de arquitectura (nov-2026). Pendiente de ejecutar; NO bloquea el
> uso de la app. Los tests deben seguir en verde (26 suites) en cada paso.

## Diagnóstico actual

| Archivo | Líneas | Problema |
|---|---|---|
| `src-tauri/src/main.rs` | ~2475 | 62 comandos `#[tauri::command]` en un solo archivo |
| `src/processor/uploader.rs` | ~1773 | Mezcla purga + reconcile + sync + info/consultas |
| `src/capture.rs` | ~1433 | `process_month` (~410) + `run_batch_history` (~660) con loop día-a-día duplicado 4 veces |
| `src/db/writer.rs` | ~1233 | Aceptable para el dominio, pero creciendo |

### Funciones >80 líneas en `main.rs`
- `sync_smart` (174) · `run_auto_sync_and_exit` (115) · `clear_db` (110) · `run_full_flow` (108) · `sync_from_last` (95)

---

## Propuesta de split

### 1. `src-tauri/src/main.rs` → módulo `commands/`

```
src-tauri/src/
├── main.rs          # Builder, invoke_handler, RunEvent (~200 líneas)
├── state.rs         # CAPTURE_STATE, caches, invalidate_dashboard_caches
└── commands/
    ├── mod.rs       # pub use de todos
    ├── dashboard.rs # get_dashboard, get_health, get_local_metrics, get_db_health
    ├── config.rs    # get/save_settings, test_supabase, is_first_run, save_first_config
    ├── capture.rs   # capture_range, sync_smart, sync_from_last, run_full_flow, abort_capture, get_capture_status
    ├── sync.rs      # upload_*, reconcile_supabase_cmd, reset_sync_marker, auto_daily_pipeline, get_next_capture_time
    ├── months.rs    # list/delete_months, import_manual_*, preview_import, reparse_raw, get_completeness, get_failed_days
    ├── audit.rs     # verify_integrity, calculate_checksums, recalculate_checksums, get_data_quality, checksum/sync history, export_nc_ndb, monitor
    ├── cartucho.rs  # export_canonical_snapshot, export_full_db, backup_db, zip_cartucho_cmd, get_last_cartucho, verify contract
    └── maintenance.rs # vacuum_db, clear_cache, clear_db, admin_reset, open_data_folder, normalize_query_id_cmd, scheduled task
```

Re-export: `mod commands; use commands::*;` — los nombres públicos no cambian → `generate_handler!` intacto.
**Esfuerzo M · Riesgo bajo** (mover sin cambiar lógica).

### 2. `src/processor/uploader.rs` → 4 módulos

| Nuevo archivo | Funciones (líneas actuales) |
|---|---|
| `uploader/purge.rs` | `delete_month_batched` (499), `delete_days_in_month_batched` (621), `purge_supabase_window` (740), `cleanup_supabase_retention` (796) |
| `uploader/reconcile.rs` | `reconcile_month` (909), `reconcile_supabase` (961), `local_month_checksum`, `resync_stale_months` |
| `uploader/sync.rs` | `upload_to_supabase`, `upload_all`, `sync_month_days`, `sync_incremental`, `check_supabase_needs_sync`, `dry_run_upload`, `verify_upload_result` |
| `uploader/info.rs` | `count_supabase_rows`, `fetch_*`, `get_supabase_info`, `test_supabase_connection` |

`uploader.rs` queda como fachada: `mod purge; mod reconcile; mod sync; mod info; pub use ...` (imports externos intactos).
**Esfuerzo M · Riesgo bajo** (extracción pura con `pub use`).

### 3. `src/capture.rs` — eliminar duplicación primero, luego dividir

1. **Extraer `download_month_dayloop(...)`**: el loop día-a-día está copiado 4 veces (~103 líneas idénticas × 4). Una sola función parametrizada.
2. Luego dividir: `capture/pipeline.rs` (run_batch_history) + `capture/month.rs` (process_month) + `capture/lock.rs` (acquire_lock, progress).

**Esfuerzo M-Med · Riesgo medio** (la duplicación es idéntica pero hay variaciones sutiles; hacer extraer + 4 reemplazos + tests).

### 4. Deduplicación entre `main.rs` ↔ `src/` (pendiente de la auditoría de limpieza)

- `last_captured_date(pool, buffer_days)` único (hoy: 4 copias del buffer −1 día) → `src/db/freshness.rs`
  - De paso corrige la **inconsistencia**: `sync_from_last` NO aplica buffer mientras `sync_smart`/`run_auto_sync` sí.
- `parse_date` (dd/mm/yyyy) → `pub` en parser y reusar en `bin/normaliza_fechas`.
- SHA-256 de manifest → helper único (2 copias).

### 5. A4 — 39 comandos sin UI (decisión requerida)

La UI minimalista invoca ~23 de 62 comandos. Opciones:
- **(a) Eliminar** los 39 no usados (mayor limpieza; git los recupera)
- **(b) Feature flag** `legacy-commands` en Cargo (compilar bajo demanda)
- **(c) Mantener** (superficie API para debugging vía devtools)

Recomendación: **(a) por lotes** — empezar por duplicados obvios: `calculate_checksums` vs `recalculate_checksums`, `sync_from_last` vs `sync_smart` vs `run_full_flow`, `upload_all` vs `upload_all_with_verify`.

---

## Orden de ejecución sugerido

1. Split `main.rs` → `commands/` (riesgo bajo, alto impacto)
2. Split `uploader.rs` → 4 módulos
3. Dedup buffer −1 + `parse_date` + SHA-256 (corrige inconsistencia de `sync_from_last`)
4. Extraer `download_month_dayloop` en `capture.rs` (quitar 3 copias)
5. A4: eliminar comandos sin UI por lotes (con test + build entre lotes)
6. Opcional: `[workspace]` raíz (un solo Cargo.lock, builds más rápidos)
