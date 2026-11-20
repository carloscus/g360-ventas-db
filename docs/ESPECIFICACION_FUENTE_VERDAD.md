# ESPECIFICACIÓN TÉCNICA — Corrección de la cadena de datos como fuente de verdad

**Repo:** `g360-ventas-db`
**Rama / worktree:** `fix-fuente-verdad` (`projects/.trees/g360-ventas-db/fix-fuente-verdad`)
**Estado:** Especificación solo (Paso spec del protocolo Nivel C). No hay código de producción ni tests nuevos aún.
**Único archivo permitido:** `docs/ESPECIFICACION_FUENTE_VERDAD.md`

---

## 1. Contexto y objetivo

El ERP CIPSA (intranet.cipsa.com.pe) es la fuente original. La base derivada SQLite (`historial.db`) y su cartucho `.db`/`.manifest.json` deben ser un **espejo exacto en el ámbito Consumo Masivo**. Actualmente hay pérdidas de datos (BDI/sub-captura), corrupción de meses por inserción diaria, duplicados sin control, checksums desactualizados y un manifiesto incompleto. Esta spec define las correcciones mínimas para restaurar el contrato v4 del consumidor.

**Supuestos (3 líneas):**
1. El ERP es la fuente original y la DB derivada debe ser espejo exacto en el ámbito Consumo Masivo.
2. Productor canónico = solo cartuchos semilla/espejo_completo, modo reemplazar; el consumidor se incrementa por su cuenta.
3. `CONTRATO.md` del consumidor (repo `g360-erp-nc-sustentor`) es guía de lectura, no autorización para modificar ese repo.

**Alcance aprobado:** código + recaptura + regeneración de cartucho. Trabajo en esta worktree. No hay commit/push/deploy aún.

---

## 2. Resolución de contradicciones entre agentes

### 2.1 insert_ventas: ¿vacía un mes o es idempotente?

**Lectura directa de `writer.rs:279-303`:**

```rust
pub async fn insert_ventas(pool, ventas, refresh_stats) -> Result<usize> {
    ...
    let mut tx = pool.begin().await?;
    if let Some(first) = ventas.first() {
        let is_daily = first.mes_ref.len() == 10
            && first.mes_ref.chars().filter(|c| *c == '-').count() == 2;
        if is_daily {
            let min_d = ventas.iter().map(|v| v.fecha_orig).min().unwrap_or(first.fecha_orig);
            let max_d = ventas.iter().map(|v| v.fecha_orig).max().unwrap_or(first.fecha_orig);
            sqlx::query("DELETE FROM ventas WHERE fecha_orig BETWEEN ? AND ?")
                .bind(min_d.to_string()).bind(max_d.to_string())
                .execute(&mut *tx).await?;
        } else {
            let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref).fetch_one(&mut *tx).await?;
            if existing > 1000 && (ventas.len() as f64) < (existing as f64) * 0.5 {
                return Err(anyhow!("Guard de integridad: {} filas nuevas < 50% ...", ...));
            }
            sqlx::query("DELETE FROM ventas WHERE mes_ref = ?")
                .bind(&first.mes_ref).execute(&mut *tx).await?;
        }
    }
    ...
}
```

**Resolver:**
- `parser.rs:338` fija `mes_ref = fecha_orig.format("%Y-%m")` → siempre 7 chars (`YYYY-MM`).
- Por lo tanto `is_daily` (`len==10`) es **código muerto** en el flujo normal: una inserción diaria nunca cumple `len==10`, cae en la rama `else` (DELETE por `mes_ref`), con el guard del 50%.
- Consecuencia real: si se inserta un día dentro de un mes que ya tiene ≥50% de filas existentes y el nuevo lote es <50% del mes, se **rechaza con error permanente** y el mes viejo se queda intacto. Si el lote es ≥50% o el mes tiene ≤1000 filas, se **borra todo el mes** y se inserta solo el día → pérdida del resto del mes.
- **Veredicto:** prevalece la lectura de **data-specialist** (puede vaciar un mes / rechazar un día). La afirmación de reviewer ("idempotente por día/mes con guard del 50%, buen diseño") es incorrecta porque no existe la rama diaria real y el guard no protege contra borrado parcial del mes.

**Decisión en la spec:** `insert_ventas` debe derivar el modo (día vs mes) del **origen real de los datos**, no de la longitud de `mes_ref`. DELETE debe operar por **rango de fechas** con **guard asimétrico** (borrar solo si el nuevo late cubre ≥X% del rango o el rango es pequeño), todo en una sola transacción.

### 2.2 tpo_doc: "NCR"/"NDB" exacto vs URL "01NCR"/"01NDB"

- `parser.rs:459-463` compara `tpo` exacto contra `"NCR"` / `"NDB"`.
- `http.rs:55` y `captor.rs` envían `valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB` al ERP.
- **Estado:** NO VERIFICADO. No se pudo ejecutar `SELECT DISTINCT tpo_doc FROM ventas` sobre la DB local (requiere acceso a la DB ejecutada / Supabase).
- **Acción en la spec:** declarar `NO VERIFICADO` y especificar el chequeo previo obligatorio antes de implementar: ejecutar `SELECT DISTINCT tpo_doc FROM ventas` sobre la DB local (modo solo lectura) y, si retorna valores con prefijo `01`, normalizar al parseo quitando el prefijo `01` antes de `derivar_campos`. Si retorna `NCR`/`NDB` limpios, no se toca.

---

## 3. Cambios file-by-file (función y comportamiento nuevo, sin código)

### 3.1 `src/browser/http.rs`
- **P0:** El defecto real no es "gate que acepta tablas parciales" sino que un grid paginado puede devolver página 1 con ≥50 filas y pasar el gate `nrows < 50`. Especificar verificación previa de si el grid pagina; si no pagina, replantear como no-prioritario. Si pagina, recorrer páginas del grid hasta no haber más en lugar del gate actual `nrows < 50` / `< 1000 B` que acepta tablas parciales como completas.
- **P2:** `danger_accept_invalid_certs(true)` mantener por compatibilidad intranet (documentado como riesgo con ticket y fecha límite de reemplazo por CA corporativa, ver §16), pero agregar comentario de por qué.
- Sin cambios en `exp_url` (los parámetros `valueDocs=01F,01B,01NCR,01NDB` se mantienen).

### 3.2 `src/browser/captor.rs`
- **P0:** `download_day_split` debe incluir un **captura separada para BDI sin filtro de artículo** (`valueArtI=` y `valueArtF=` vacíos) cuando el modo sea día y se detecten documentos BDI.
- **P0:** Contar filas por bucket y comparar contra la descarga sin split del mismo día. La referencia de conteo sin split es la ruta existente `captor.rs:230` (`download_xls`, `accion=D0` con `valueArtI=` vacío) — convertir el XLS/CSV de esa descarga y contar filas (o leer `nrows` del grid del ERP). Si el día fusionado tiene menos filas que la descarga sin split, **abortar con error** (no insertar nada).
- **P0:** Si la descarga sin split falla, abortar igualmente (no se puede validar integridad sin la referencia de conteo).
- **P2:** logging de conteo BDI por bucket para detectar pérdida.

### 3.3 `src/capture.rs`
- **P0:** El dedup semántico se hace ÚNICAMENTE en `insert_ventas` por la 4-tupla `(folio_unico, id_articulo, cantidad, soles)` (fila completa). `merge_csvs` permanece como merge puro de líneas de texto (`capture.rs:52` y `:88`), SIN dedup. El parser emite una fila por línea de artículo (`parser.rs:471`); una factura puede tener varias filas con el mismo `(folio_unico, id_articulo)` pero distinto `cantidad`/`soles` (legítimas). `(folio_unico, id_articulo)` es CLAVE DE AGRUPACIÓN (`refresh_dim_documento` agrupa por `folio_unico`, `writer.rs:793`), nunca de unicidad. `dim_documento.folio_unico` YA es PRIMARY KEY (`schema.rs:304`): no requiere migración.
- **P0:** Al insertar, conservar la primera aparición de cada 4-tupla y descartar el resto con log de advertencia. `merge_csvs` debe conservar las líneas legítimas de un mismo documento en buckets distintos (no fusionar por `folio_unico`).
- **QUITAR:** la migración "UNIQUE en `ventas.folio_unico"` y el test "no hay folio_unico duplicado" — ambos fallarían con facturas legítimas multi-línea (mismo `folio_unico`, distinto `id_articulo`).
- **P0:** `process_month` no debe deglutir errores de parseo (`if let Ok` → propagar `Err`). Un parseo fallido debe marcar el mes como fallido, no como éxito con 0 filas.
- **P1:** Agregar métrica `capture_dropped_rows` separada de `data_quality.total_records`: conteo de filas leídas vs filas insertadas por día/mes, expuesta en el progreso y en el manifiesto. Ver §13 para formato exacto.

### 3.4 `src/processor/parser.rs`
- **P1:** `parse_f64` NO forzar "1.234"→1234 globalmente. Acotar solo a columnas monetarias SOLES/DOLARES. Regla exacta tipo `\d{1,3}(\.\d{3})+` (punto como separador de miles, sin coma decimal) solo cuando la columna es monetaria. Mostrar muestras reales del ERP como evidencia previa antes de aplicar.
- **P1:** `parse_f64_ctx` (y `parse_f64` para columnas no monetarias) debe mantener `"1.500"` → 1.5 para CANTIDAD y PRECIO UNITARIO (regla: `.` es decimal en cantidades, nunca miles). Test de no-regresión: `parse_f64_ctx("1.500", "CANTIDAD") == 1.5`, `parse_f64_ctx("1.234", "SOLES") == 1234`.
- **P1:** `derivar_campos` debe normalizar `tpo_doc` quitando prefijo `01` si existe (`01NCR` → `NCR`, `01NDB` → `NDB`) antes de clasificar `tipo_operacion` y construir `folio_unico`.
- **P1:** Filas descartadas (fecha inválida, futura, sin ID) deben contabilizarse en `QualityStats` y no silenciarse. `total_rows` YA incluye descartadas (`parser.rs:427` suma invalid+future+empty). Quitar el cambio propuesto que decía "total_rows no incluye descartadas"; el hueco real es `capture_dropped_rows` (sí válido, ver §13).
- Sin cambios en la firma pública de `parse_export_csv`.

### 3.5 `src/processor/xls.rs`
- **P2:** `inspect_sku_column` ya existe y se usa en `importxls`. Mantener; asegurar que el guard de SKU numérico se aplique también en la ruta de captura automática (no solo import manual).

### 3.6 `src/processor/uploader.rs` + `uploader/{sync,reconcile,purge,info}.rs`
- **P1:** `contracto_version` emitido como **entero 4** en el manifiesto (actualmente string `"4"`). Corregir en `export_canonical_snapshot` y `build_initial_cartucho`. Test de entero en `export_canonical_snapshot` Y `build_initial_cartucho`. Definir esquema JSON canónico v4 (ver §11).
- **P1:** `last_sync` parametrizado: no interpolar string en SQL. Usar parámetros bind en `sync.rs:151,599` (usar `format!("WHERE capturado_en > '{}'", s)` → bind) y extender a `reconcile.rs:149/223/404` (`where_clause` con `format!` interpola `mes_esc` y `mes` sin bind).
- **P1 (cobertura de interpolación):** los sitios restantes de interpolación de cadena en SQL se inventarían como "valores internos, no usuario" para que no parezca cobertura incompleta: `sync.rs:168/170/177/203/327/334/495-504/606/608/612/634/646` y `reconcile.rs:403-410`. Ninguno interpola entrada de usuario; todos usan constantes o valores ya validados internamente. Quedan documentados como no-riesgo de inyección.
- **P1 (prueba del bind):** verificar que `last_sync` y cualquier otro valor viajan como parámetros, nunca interpolados, en `sync.rs`, `reconcile.rs`, `purge.rs` e `info.rs`. Nota: `sync.rs:495` y `reconcile.rs:404` ya escapan con `.replace('\'',"''")`, pero `sync.rs:151/599` y `reconcile.rs:149/223` no — el mandato de bind cubre todos por igual.
- **P1:** `get_supabase_service_key()` fallback silencioso a anon: eliminar fallback. Si falta service_role key, fallar con error claro en operaciones de escritura.
- **P1:** Alinear `contenido` del manifiesto con `allowed_lines` y `supabase_retention_days`: si el uploader filtra líneas, `contenido` no puede decir "TODAS las líneas del ERP". Cambiar a describir exactamente lo que lleva el cartucho (raw completo local) y separar el campo `supabase_content` (filtrado) del `contenido` (raw completo).
- **P2:** `supabase_retention_days=1277` contradice `contenido` ("sin retención"). Unificar: `contenido` describe raw completo local; `retention_days` describe solo Supabase.

### 3.7 `src/db/{schema,writer,freshness}.rs`
- **P0:** `schema.rs:69,203`: `folio_unico` sin constraint única en `ventas`. **NO agregar `UNIQUE` en `ventas.folio_unico`** (fallaría con facturas legítimas multi-línea: mismo folio, distinto `id_articulo`). La clave de dedup es la 4-tupla `(folio_unico, id_articulo, cantidad, soles)`; ya existe índice `idx_venta_folio_articulo ON ventas(folio_unico, id_articulo)` (`schema.rs:183`) como soporte de agrupación, no de unicidad. `dim_documento.folio_unico` YA es PRIMARY KEY (`schema.rs:304`) — declarar que **no requiere migración**.
- **P0:** `refresh_dim_documento` agrupa con `COUNT(*) AS n_lineas` (`writer.rs:793`) por `folio_unico` (una fila por documento, varias líneas).
- **QUITAR:** migración "UNIQUE en `ventas.folio_unico"` y el test "no hay folio_unico duplicado" (fallaría con facturas legítimas multi-línea).
- **P0:** `writer.rs:285-303` (`insert_ventas`): reescribir para derivar modo día/mes del origen real, DELETE por rango de fechas, guard asimétrico, transacción única. Ver sección 4. Enum explícito `ModoInsert { Dia, Mes }`.
- **P0:** `writer.rs:130-138`: `ALTER TABLE ADD CHECK` con `.ok()` son no-ops en SQLite. Reemplazar por migración explícita que verifique la creación del CHECK y falle si no se puede aplicar (o documentar que los CHECKs solo aplican en DB nueva vía `CREATE_TABLE_SQL`).
- **P0:** Recalcular `mes_checksums` y `day_checksums` después de cada inserción/recaptura, **antes** de `VACUUM INTO`. Refrescar `dim_*` y `fact_venta_mes` antes del VACUUM.
- **P2:** `verify_deploy` debe incluir `dim_ruc` en la lista de tablas críticas — YA lo hace en la lista de nombres (`writer.rs:711` itera `["ventas","dim_ruc",...]`). Corregir el comentario inexacto que decía que no lo incluía en la lógica real.
- **P2:** `soles_total` redondeado a 2 decimales en manifiesto y checksums.
- **P0/P1:** `build_initial_cartucho` emite manifiesto incompleto (falta `day_checksums`, `data_quality`, `attributes_applied` completo, `soles_total`, `fecha_min/fecha_max`). Unificar con `export_canonical_snapshot`.
- **P0:** Ampliar inventario de `build_initial_cartucho` con `allowed_lines`, `supabase_retention_days`, `quick_check.soles`, refresco dims/fact antes del VACUUM.
- **P0:** Añadir invariante `Σ day_checksums.filas == quick_check.filas` a §8 (verificación de equivalencia).

### 3.8 `src/models.rs`
- No se prevé cambio de estructura de `Venta`. Solo documentar que la clave canónica de FILA es la 4-tupla `(folio_unico, id_articulo, cantidad, soles)` y que `folio_unico` identifica el documento, no la fila.

### 3.9 `src/config.rs`
- **P2:** `.env.example`: `G360_INTRANET_USER=ccusi` (usuario real) → cambiar a placeholder `G360_INTRANET_USER=tu_usuario_intranet`. `G360_INTRANET_PASS=tu_password` ya es placeholder.
- **P2:** `SUPABASE_URL`/`SUPABASE_KEY` declaradas pero el código nunca las lee (usa `config.enc`/config.json). Documentar o eliminar del `.env.example`.

### 3.10 `src/bin/{export,build_initial_cartucho,capture_daily,upload,reparse,recapture}.rs`
- `capture_daily`: debe disparar recalculo de checksums y refresco de dims/fact antes de finalizar.
- `reparse`: usar el nuevo `insert_ventas` (modo día/mes del origen real) y propagar errores de parseo.
- `recapture`: mismo comportamiento; al finalizar, regenerar cartucho automáticamente (o marcar pendiente).
- `upload`: usar `last_sync` parametrizado (bind), sin fallback silencioso a anon. Extender parametrización también a `reconcile.rs:149/223/404` (ver §3.6).
- `export` / `build_initial_cartucho`: generar manifiesto completo (ver 3.6) y `VACUUM INTO` solo después de checksums+dims frescos.
- `total_rows` YA incluye descartadas (`parser.rs:427` suma invalid+future+empty). NO cambiar ese comportamiento. El hueco real es `capture_dropped_rows` (sí válido, ver §13).

### 3.11 `tests/*.rs`
- **P0:** Test de conteo por bucket + abort si día fusionado < descarga sin split (fixture sintético).
- **P0:** Test de `insert_ventas` modo día con guard asimétrico y DELETE por rango.
- **P0:** Test de dedup por 4-tupla `(folio_unico, id_articulo, cantidad, soles)` en `insert_ventas` (no en `merge_csvs`, que no deduplica).
- **P1:** Test `contracto_version` entero (no string) en manifiesto.
- **P1:** Test de no-regresión `parse_f64_ctx("1.500","CANTIDAD") == 1.5` y `parse_f64_ctx("1.234","SOLES") == 1234`.
- **P1:** Test `derivar_campos` con `tpo_doc="01NCR"` → tipo_operacion correcto.
- Arreglar fixture `tests/raw_first_filter.rs`: agregar columna `ID_ARTICULO` al CSV (requerido en `parser.rs:288-301`).
- Ejecutar las 4 suites no corridas: `reparse_filtro`, `sku_column_guard`, `vw_comparativo`, `vw_historial_articulo`.
- No usar `it.skip`, `assert true` ni tests borrados.

### 3.12 `.env.example`
- Placeholder para `G360_INTRANET_USER` (no usuario real).
- Mantener `SUPABASE_URL`/`SUPABASE_KEY` con valores dummy si se documentan como legacy.

### 3.13 `frontend/index.html`
- **P2:** Sacar input de `service_role key` del frontend (líneas 671, 912). El service role nunca debe exponerse al navegador. Solo `supabase_url` y `supabase_key` (anon) en UI.

### 3.14 `scripts/verify_rls_policies.sql`
- **P2:** Agregar aserciones (conteo esperado > 0) y **negative tests** (intentar `INSERT` como `anon` y verificar que falle; intentar `SELECT` en `ventas` como `anon` y verificar que falle si RLS lo restringe).

### 3.15 `scripts/supabase_quick_check.sql`
- Mantener; agregar chequeo de `contracto_version` entero si aplica al contexto Supabase.

### 3.16 `supabase/migrations/*`
- **P2:** Resolver contradicción `docs/CONTRATO.md:224` (anon SELECT) vs `rls_consolidated.sql:95` (`REVOKE ALL FROM anon`). Elegir una política consistente: anon solo SELECT en vistas, sin acceso a tabla `ventas`. Actualizar migración y docs para que coincidan.
- `dim_documento.folio_unico` YA es PRIMARY KEY (`schema.rs:304`), por lo que **no requiere migración de UNIQUE**. Declarar en la migración: "UNIQUE en `dim_documento.folio_unico` ya garantizado por PRIMARY KEY — no ejecutar nada".
- **PROHIBIDO textualmente:** aplicar `UNIQUE` a `ventas.folio_unico` (es justo el defecto que se corrigió: fallaría con facturas legítimas multi-línea, mismo `folio_unico` y distinto `id_articulo`). CHECKs en `ventas` sí, conforme a §15.

### 3.17 `docs/BDI_INVESTIGACION.md`
- Actualizar estado: confirmar hipótesis (BDI se pierde con split por artículo) y registrar la fix (captura separada BDI sin filtro de artículo).

### 3.18 `docs/CONTRATO.md`
- No modificar (lectura solo). Referenciar como guía del consumidor v4.

### 3.19 `README.md`
- Actualizar comando de verificación para que coincida con `AGENTS.md` (`cargo build && cargo test`) o documentar ambas formas. Actualizar sección de cartucho si cambia el manifiesto.

---

## 4. Diseño de `insert_ventas` (decisión de modelo de datos y clave de dedup)

**Clave de dedup canónica:** fila completa / 4-tupla `(folio_unico, id_articulo, cantidad, soles)` — NO `folio_unico` solo, NO `(folio_unico, id_articulo)` como unicidad. `folio_unico = "{tpo_doc}/{serie_doc}/{nro_doc}"` identifica el documento; una misma factura tiene varias líneas con distinto `id_articulo`, y puede tener varias filas con el mismo `(folio_unico, id_articulo)` pero distinto `cantidad`/`soles` (líneas legítimas del mismo documento con distinta cantidad/precio). La clave de fila es la 4-tupla completa. `(folio_unico, id_articulo)` queda como CLAVE DE AGRUPACIÓN (por ejemplo `refresh_dim_documento` agrupa `COUNT(*) AS n_lineas` por `folio_unico`, `writer.rs:793`), nunca como clave de unicidad. El índice existente `idx_venta_folio_articulo ON ventas(folio_unico, id_articulo)` (`schema.rs:183`) soporta la agrupación; `dim_documento.folio_unico` es PRIMARY KEY (`schema.rs:304`, no requiere migración). El dedup de filas exactas se hace únicamente en `insert_ventas` por la 4-tupla `(folio_unico, id_articulo, cantidad, soles)`; `merge_csvs` no deduplica (merge puro de líneas de texto).

**Supuesto alternativo (solo si se acepta):** si se declara explícitamente que el ERP emite a lo sumo una línea por `(documento, artículo)`, entonces `(folio_unico, id_articulo)` podría usarse como clave de unicidad. No es el caso por defecto: la opción recomendada y la que aplica en esta spec es la 4-tupla, para no borrar datos válidos.

**Inventario de call sites de `insert_ventas`:**
- `capture.rs:35` — `insert_with_retry` llama a `insert_ventas(pool, ventas, false)` (INDIRECTO: llamado desde `process_month`, que a su vez es invocado por `run_batch_history`).
- `bin/reparse.rs:42` — llama `insert_ventas(&pool, &ventas, false)` DIRECTAMENTE.
- `bin/importxls.rs:66` — llama `insert_ventas(&pool, &ventas, true)` DIRECTAMENTE (con `refresh_stats=true`). **Al cambiar la firma de `insert_ventas` agregando `ModoInsert`, este call site se rompe y debe actualizarse.**
- `bin/capture_daily.rs:76` — NO llama `insert_ventas`: delega en `capture::run_batch_history` → `process_month` → `insert_with_retry` → `insert_ventas` (INDIRECTO).
- `bin/recapture.rs:67` — NO llama `insert_ventas`: delega en `capture::run_batch_history` → `process_month` → `insert_with_retry` → `insert_ventas` (INDIRECTO).
- `capture.rs` `parse_export_csv_with_cross` call sites: líneas 377, 446, 505, 618, 653 (cada uno pasa el lote parseado a `insert_with_retry` → `insert_ventas`; INDIRECTO vía `insert_with_retry`).
- Todos deben recibir el `ModoInsert` correcto del origen de los datos.

**Enum explícito:**
```rust
pub enum ModoInsert { Dia, Mes }
```
`insert_ventas` recibe `modo: ModoInsert` derivado del **origen real de los datos** (captura día-a-día → `Dia`; descarga mensual → `Mes`). No se infiere de `mes_ref.len()`.

**Nuevo comportamiento de `insert_ventas`:**
1. Recibe lote de `Venta` + `modo: ModoInsert`.
2. **Modo Día:** `DELETE FROM ventas WHERE fecha_orig BETWEEN min_d AND max_d` (rango del lote), luego INSERT. Guard asimétrico: si el rango tiene >1000 filas existentes y el lote nuevo es <50% de ellas, **rechazar con error** (no borrar).
3. **Modo Mes:** `DELETE FROM ventas WHERE mes_ref = ?`, luego INSERT. Mismo guard asimétrico.
4. Todo en **una sola transacción**. Si falla el INSERT, se hace rollback y no hay DELETE parcial.
5. `refresh_stats` opcional; tras commit, llamar a `calculate_monthly_checksums` + `calculate_day_checksums` + `refresh_dim_tables` + `refresh_fact_venta_mes` antes de cualquier `VACUUM INTO`.

**Justificación:** El mes no debe quedar vacío porque un día se re-captura con menos filas. El guard asimétrico evita borrar un mes completo por un lote parcial. La clave de dedup (4-tupla `(folio_unico, id_articulo, cantidad, soles)`) permite detectar duplicados reales sin bloquear inserciones legítimas de varias líneas del mismo documento.

---

## 5. Plan de implementación por fases (dependencias)

**Fase A — Parser y modelo (no requiere DB ni Supabase):**
1. `parser.rs`: `parse_f64` acotado a columnas monetarias SOLES/DOLARES (regla `\d{1,3}(\.\d{3})+`), `derivar_campos` normaliza `01NCR/01NDB`, contabilizar filas descartadas. Test de no-regresión `parse_f64_ctx("1.500","CANTIDAD") == 1.5`.
2. `capture.rs`: `merge_csvs` NO deduplica (merge puro de líneas de texto); propagar errores de parseo; métrica `capture_dropped_rows`. El dedup semántico va en `insert_ventas` por 4-tupla (ver §3.3).
3. `http.rs`/`captor.rs`: captura BDI sin filtro de artículo; verificación de paginación del grid; conteo por bucket + abort.
4. Tests: `parse_f64` (CANTIDAD/PRECIO UNITARIO no-regresión), `derivar_campos` con `01NCR`, dedup por 4-tupla `(folio_unico, id_articulo, cantidad, soles)` en `insert_ventas`, conteo por bucket + abort.

**Fase B — Escritura y DB (requiere DB local para verificar, no Supabase):**
1. `writer.rs`: nuevo `insert_ventas` (modo día/mes del origen, DELETE por rango, guard asimétrico, transacción única, enum `ModoInsert { Dia, Mes }`).
2. Actualizar TODOS los call sites a la nueva firma `insert_ventas(..., ModoInsert)`: `capture.rs:35` (directo), `bin/reparse.rs:42` (directo), `bin/importxls.rs:66` (directo, `refresh_stats=true` — se rompe en compilación si no se actualiza), y los indirectos `capture_daily`/`recapture` vía `capture::run_batch_history` → `process_month` → `insert_with_retry`. Cada uno debe pasar el modo derivado del origen real (día vs mes), no `len(mes_ref)`.
3. `schema.rs`: **NO agregar UNIQUE en `ventas.folio_unico`** (fallaría con multi-línea); `dim_documento.folio_unico` ya es PK (no requiere migración).
4. `writer.rs`: CHECKs no son no-ops (migración explícita o documentación).
5. `writer.rs`: recalcular checksums + refrescar dims/fact antes de `VACUUM INTO`.
6. Tests: `insert_ventas` modo día/mes con guard, CHECKs aplicados, checksums frescos.

**Fase C — Cartucho y manifiesto (requiere DB local):**
1. `export_canonical_snapshot` + `build_initial_cartucho`: manifiesto completo (contracto_version entero, day_checksums, data_quality, attributes_applied, soles_total, fecha_min/max).
2. `config.rs`: `.env.example` placeholders.
3. Tests: manifiesto v4 completo, quick_check coincide con DB.

**Fase D — Supabase y seguridad (requiere Supabase para verificar):**
1. `sync.rs`/`reconcile.rs`/`purge.rs`/`info.rs`: `last_sync` bind, eliminar fallback anon, `contracto_version` entero.
2. `frontend/index.html`: sacar service_role key.
3. `scripts/verify_rls_policies.sql`: aserciones + negative tests.
4. `supabase/migrations/`: resolver contradicción anon/RLS; verificar que `dim_documento.folio_unico` es PK en Supabase (paridad con `schema.rs:304`) sin constraint nuevo; UNIQUE en `ventas.folio_unico` está PROHIBIDO (rompe facturas multi-línea).
5. No se puede verificar sin Supabase (ver sección 7).

**Fase E — Recaptura y cartucho (ejecución, post-aprobación):**
1. Recaptura días afectados desde 16/11/2026.
2. Regenerar cartucho con rollback (ver sección 6).
3. Ejecutar `cargo build && cargo test` + `g360 audit .`.

---

## 6. Estrategia de recaptura + regeneración de cartucho + rollback (inventario, no ejecución)

**Inventario de días afectados:** desde 16/11/2026 (fecha del hallazgo BDI: 2026-11-16, intranet 590 filas/64 BDI vs cartucho 250 filas/2 BDI).

**Inventario de tablas y artefactos por día/mes:**
- `ventas`: filas raw del día/mes capturado.
- `day_checksums`: una fila por `fecha_orig` con `filas`, `soles`, `checksum`.
- `mes_checksums`: una fila por `mes_ref` con `checksum`, `total_filas`, `total_soles`, `total_cantidad`.
- `dim_*` / `fact_venta_mes`: `dim_cliente`, `dim_vendedor`, `dim_articulo`, `dim_documento`, `dim_linea`, `fact_venta_mes` (refrescar antes del VACUUM).
- `g360_meta`: `contracto_version`, `normalizacion_version`, `user_version`.
- Manifiesto maestro: `.manifest.json` (esquema canónico v4, ver §11).
- Artefactos raw: CSV/XLS descargados en `data/raw/`, cartuchos `.db` en `staging/` y `release/`.

**Orden de operaciones + aserciones post-migración:**
1. Backup del `.db` actual antes de tocar nada.
2. Recaptura días afectados (nuevo motor: BDI sin split, conteo por bucket, abort si pérdida).
3. Verificar conteo por día: ERP ↔ SQLite local (conteo igual).
4. Recalcular `day_checksums` y `mes_checksums` para los meses tocados.
5. Refrescar `dim_*` y `fact_venta_mes`.
6. Aserción de equivalencia: `Σ day_checksums.filas == quick_check.filas == COUNT(*) FROM ventas`.
7. `export` → `VACUUM INTO` → `.db` + `.manifest.json` completo en `staging/`.
8. Verificar manifiesto: `quick_check.filas == COUNT(*)`, `day_checksums` es array de objetos de una clave `[{ "2026-11-16": { filas: 590, soles: <float>, checksum: "<str>" } }]` (forma declarada en §11), acceso `day_checksums[0]["2026-11-16"].filas == 590` (esperado), `contracto_version == 4` (entero).
9. Tests de equivalencia (§8) + firma humana.
10. Copiar cartucho de `staging/` a `release/` y actualizar manifiesto maestro.

**Gate de quarantine (obligatorio):**
- El cartucho nuevo se genera en `staging/` y **NO se publica** hasta pasar los tests de equivalencia (§8) + **firma humana**.
- Solo tras pasar ambos se copia a `release/` y se actualiza el manifiesto maestro.
- **Punto de no retorno:** una vez copiado a `release/` y firmado, el cartucho es el vigente y no se puede retroceder sin restaurar el backup del paso 1.
- El cartucho vigente actual (250 filas el 16/11) se marca como **NO DISTRIBUIBLE** ya mismo (no se envía a nuevos clientes ni se reemplaza en release hasta que el nuevo pase el gate).

**Rollback:**
- Antes de recapturar, hacer backup del `.db` actual (`backup_db` o copia de archivo).
- Si la recaptura falla o el conteo no cuadra, restaurar el `.db` desde backup y no distribuir el cartucho nuevo.
- El lock `capture.lock` evita capturas concurrentes durante el proceso.
- Si `VACUUM INTO` falla, el `.db` original queda intacto (VACUUM INTO escribe a ruta distinta).

---

## 7. Qué NO se puede verificar sin Supabase ni sin ejecutar la DB

- `SELECT DISTINCT tpo_doc FROM ventas` para confirmar si hay prefijo `01NCR/01NDB` (declarado NO VERIFICADO, chequeo previo obligatorio).
- `upload_to_supabase`, `reconcile_supabase`, `verify_upload_result` (requieren red y Supabase).
- RLS policies, `verify_rls_policies.sql` negative tests (requieren Supabase).
- `g360 audit .` checks que tocan Supabase (si los hay).
- `cargo test` suites que necesitan DB real (las suites en `tests/` usan `sqlite::memory:` y sí corren local).

**Chequeo previo obligatorio del agente designado antes de tocar código:**
1. Ejecutar `SELECT DISTINCT tpo_doc FROM ventas` sobre la DB local (solo lectura) y registrar los valores distintos.
2. Si hay valores con prefijo `01`, incluir la normalización en `parser.rs`; si no, no tocar `derivar_campos` para ese punto.
3. Confirmar que `cargo build && cargo test` pasa en la rama actual (línea de base) antes de aplicar cambios.
4. Confirmar que existe `.env.example` y que no hay secretos reales versionados.

---

## 8. Plan de tests de equivalencia

| Test | Qué compara | Qué verifica |
|------|-------------|--------------|
| conteo_por_dia | ERP ↔ SQLite ↔ cartucho ↔ Supabase | filas por día iguales en los 4 lados (donde Supabase esté disponible) |
| integridad_documental | 01F / 01B / 01NCR / 01NDB | cada tipo de documento presente y con conteo esperado |
| dedup_fila_4tupla | `ventas` local | no hay 4-tupla `(folio_unico, id_articulo, cantidad, soles)` duplicada (mismo folio con distinto `id_articulo` es válido: multi-línea; misma 4-tupla es duplicado real) |
| checksums_vivo | `.db` exportado | `day_checksums` y `mes_checksums` coinciden con `GROUP BY` en vivo sobre el `.db`; invariante explícita `Σ day_checksums.filas == quick_check.filas == COUNT(*) FROM ventas` (ver §6 paso 6 y §11) |
| derivados_dim_fact | `dim_*`, `fact_venta_mes` | consistentes con `ventas` (refrescados antes de VACUUM) |
| tipo_operacion_vs_tpo_doc | `derivar_campos` | NCR+cant<0 → devolucion; NCR+cant>=0 → ajuste_valor; NDB → ajuste_valor; F01/BDI → venta |
| idempotencia_reparse | reparse del mismo CSV dos veces | mismo conteo, sin duplicados |
| idempotencia_resync | upload del mismo mes dos veces | sin duplicados en Supabase |
| fixture_ID_ARTICULO | `tests/raw_first_filter.rs` | CSV de test incluye `ID_ARTICULO` y las 6 filas se parsean |

**Suites no corridas que deben ejecutarse:** `reparse_filtro.rs`, `sku_column_guard.rs`, `vw_comparativo.rs`, `vw_historial_articulo.rs`.

---

## 9. Riesgos y criterios de abort

**Riesgos:**
- Pérdida de datos si `insert_ventas` borra mes y el lote nuevo es incompleto (mitigado por guard asimétrico).
- Captura parcial por servidor intranet caído (mitigado por retry + day-a-day + HTML fallback).
- Duplicados si `insert_ventas` no deduplica antes de insertar (mitigado por dedup por 4-tupla `(folio_unico, id_articulo, cantidad, soles)`; `folio_unico` solo NO es clave de fila porque una factura tiene varias líneas).
- Manifiesto incompleto → consumidor rechaza cartucho (mitigado por manifiesto completo v4).
- Service role key expuesta en frontend (mitigado por sacar el input).
- Backup local del `.db` sin cifrar (datos de ventas crudos, negocio-sensible; §6 paso 1): riesgo aceptado con acciones mínimas — restringir permisos del archivo de backup, guardarlo fuera del repo, no versionarlo.

**Criterios de abort (no continuar con la implementación si ocurre alguno):**
- `cargo build && cargo test` en rojo en la línea de base antes de empezar.
- `SELECT DISTINCT tpo_doc FROM ventas` no ejecutable y no hay forma de confirmar prefijo `01` (bloquea la corrección de `derivar_campos`).
- Acceso a Supabase no disponible y se requiere verificar RLS/upload (esos ítems se dejan para Fase D).
- Cualquier cambio que requiera migración de datos en Supabase (no está en alcance; es responsabilidad del equipo de Supabase).

---

## 10. Aprobaciones humanas que faltan antes de implementar

1. **Producto / dueño del contrato v4:** confirmar que `contracto_version` entero 4, `tipo=semilla`, `espejo_completo=true`, `incremental=false`, modo reemplazar son innegociables para el consumidor.
2. **Dueño de la DB local:** confirmar que **NO agregar `UNIQUE` a `ventas.folio_unico`** (clave de dedup es la 4-tupla `(folio_unico, id_articulo, cantidad, soles)`; `(folio_unico, id_articulo)` solo es clave de agrupación), CHECKs a `ventas` y que `dim_documento.folio_unico` ya es PK (no migración) no rompe instalaciones existentes.
3. **Dueño de Supabase:** confirmar política RLS (anon SELECT solo en vistas, REVOKE ALL en tabla `ventas`, ver §14) y disponibilidad para verificar `verify_rls_policies.sql`.
4. **Dueño del intranet CIPSA:** confirmar que la captura BDI sin filtro de artículo (`valueArtI=` y `valueArtF=` vacíos) está permitida y no sobrecarga el servidor.
5. **Dueño de la recaptura:** autorizar la ejecución de `recapture -- 2026-11-16 ...` y el backup/rollback del `.db` antes de distribuir el cartucho nuevo.
6. **Dueño de seguridad/cartoncho (nuevo por gate de quarantine):** autorizar el gate de quarantine (§6): cartucho en `staging/` hasta pasar tests de equivalencia + firma humana, copia a `release/` y actualización de manifiesto maestro; marcar cartucho vigente (250 filas el 16/11) como NO DISTRIBUIBLE.

**SPEC_ESCRITA: encabezado, contexto, resolución de contradicciones (insert_ventas y tpo_doc), cambios file-by-file P0/P1/P2, diseño de insert_ventas y clave dedup, plan por fases, recaptura+rollback con gate de quarantine, tests de equivalencia, riesgos/criterios de abort, aprobaciones pendientes — contradicciones resueltas: sí — aprobaciones pendientes: 6**

---

## 11. Esquema JSON canónico del manifiesto v4

**Migración del manifiesto ya distribuido:** tiene `"contracto_version": "4"` como string (línea 38). Corregir a entero `4` en `export_canonical_snapshot` y `build_initial_cartucho`. Test de entero en ambos.

**Esquema canónico v4:**
```json
{
  "contracto_version": 4,
  "dim_schema_version": "<string>",
  "normalizacion_version": "<string>",
  "format": "g360-ventas-cartucho-v2",
  "tipo": "semilla",
  "espejo_completo": true,
  "incremental": false,
  "generado_en": "<ISO 8601>",
  "generado_por": "<string>",
  "rango": { "desde": "<fecha>", "hasta": "<fecha>" },
  "db": {
    "archivo": null,
    "sha256": "<hex>",                 // única fuente del hash del .db (sha256 de nivel raíz eliminada: duplicada)
    "bytes": <int>,                     // única fuente del tamaño (db_size_bytes de nivel raíz eliminada: duplicada)
    "ventas_filas": <int>,              // única fuente de conteo de ventas (db.filas / quick_check.filas unificadas aquí; quick_check.filas == ventas_filas por invariante)
    "filas": <int>,                     // alias de ventas_filas; mantener iguales por invariante Σ day_checksums.filas == quick_check.filas == ventas_filas
    "soles_total": <float redondeado 2 decimales>,
    "fecha_min": "<fecha>",
    "fecha_max": "<fecha>"
  },
  "allowed_lines": "<lista o descripción de filtros aplicados>",
  "supabase_retention_days": <int>,
  "supabase_content": "<descripción de lo filtrado para Supabase>",
  "sha256": "<hex del .db>",              // alias de db.sha256; única fuente es db.sha256, mantener iguales
  "db_size_bytes": <int>,               // alias de db.bytes; única fuente es db.bytes, mantener iguales
  "quick_check": {
    "ok": true,
    "filas": <int>,                     // == db.ventas_filas (invariante Σ day_checksums.filas == quick_check.filas == ventas_filas)
    "soles": <float>,
    "meses": <int>
  },
  "data_quality": {
    "status": "<excellent|good|needs_review|unknown>",
    "total_records": <int>,
    "valid_records": <int>,
    "invalid_records": <int>,
    "invalid_reasons": {
      "invalid_dates": <int>,
      "future_dates": <int>,
      "empty_ids": <int>
    },
    "capture_dropped_rows": <int>,
    "price_unitario_status": "<ok|needs_recalc>",
    "price_units_recalc_needed": <int>
  },
  "attributes_applied": ["id_pedido_normalized", "mes_ref_from_fecha_orig", ...], // condicional: solo los realmente aplicados
  "dias": ["<fecha>", ...],
  "day_checksums": [{ "<fecha>": { "filas": <int>, "soles": <float>, "checksum": "<str>" } }],
  "mes_checksums": [{ "mes_ref": "<str>", "checksum": "<str>", "filas": <int>, "soles": <float> }],
  "contenido": "<descripción exacta de lo que lleva el cartucho (raw completo local)>"
}
```
- `data_quality` se lee desde la DB (no del parser en memoria).
- `attributes_applied` es condicional: solo lista los atributos realmente aplicados en este cartucho.
- Sin `retention_days` legacy en `contenido`; `retention_days` describe solo Supabase.

---

## 12. `capture_dropped_rows` — exposición y formato

- Se expone **dentro de `data_quality`** del manifiesto (campo `capture_dropped_rows`), separado de `total_records`.
- Formato exacto: entero ≥ 0, representa `filas_leídas - filas_insertadas` por día/mes (conteo de filas leídas del CSV/XLS menos filas efectivamente insertadas en `ventas`).
- Se calcula en `capture.rs` al hacer `merge_csvs` + `parse_export_csv_with_cross` + `insert_ventas`, y se reporta en el progreso y en el manifiesto.

---

## 13. tpo_doc — precheck obligatorio (Fase A)

- Hacer el `SELECT DISTINCT tpo_doc FROM ventas` **PRECHECK OBLIGATORIO** antes de implementar la normalización de prefijo `01`.
- **Bloquear Fase A** si no hay evidencia del resultado de ese `SELECT`.
- Si retorna valores con prefijo `01` (ej. `01NCR`, `01NDB`), normalizar en `parser.rs` quitando el prefijo antes de `derivar_campos`. Si retorna `NCR`/`NDB` limpios, no se toca.
- Estado actual: NO VERIFICADO (requiere acceso a la DB local ejecutada).

---

## 14. RLS — decisión documentada

- Decisión explícita sobre la contradicción `docs/CONTRATO.md:224` (anon SELECT) vs `rls_consolidated.sql:95` (`REVOKE ALL FROM anon`): **anon solo SELECT en vistas, sin acceso a tabla `ventas`; REVOKE ALL en tabla `ventas` prevalece.**
- Actualizar migración y docs para que coincidan.
- **Negative test en CI:** intentar `INSERT` como `anon` y verificar que falle; intentar `SELECT` en `ventas` como `anon` y verificar que falle.

---

## 15. CHECKs — validación pre-migración y reparación

- Validación pre-migración de datos violadores: ejecutar consulta que detecte filas de `ventas` que violarían los CHECKs antes de aplicar la migración.
- Script de reparación para DBs existentes: corregir o marcar las filas violadoras antes de aplicar los CHECKs (no aplicar CHECKs sobre datos sucios).
- Documentar en migración qué CHECKs aplican solo en DB nueva vía `CREATE_TABLE_SQL` vs cuáles requieren migración explícita.

---

## 16. `danger_accept_invalid_certs(true)` — riesgo registrado

- Riesgo registrado con ticket y fecha límite de reemplazo por CA corporativa.
- Mantener por compatibilidad intranet, agregar comentario de por qué, y ticket con fecha límite para reemplazar por CA corporativa.
- **Transporte HTTP sin TLS:** `src/browser/http.rs:13-14` y `src/config.rs:48-51` usan `http://intranet.cipsa.com.pe`; el login POST (`http.rs:72-88`) envía usuario/contraseña en texto claro. `danger_accept_invalid_certs(true)` está latente (solo aplica si el intranet migra a HTTPS con cadena inválida). Registrar en el MISMO ticket de CA corporativa con la migración a HTTPS como fecha límite; hasta entonces, riesgo aceptado documentado. No se agrega gate de abort: el transporte no cambia en esta recaptura (no hay downgrade nuevo).

---

## 17. Auditoría de secretos + lint de `.env.example`

- Auditoría de secretos: confirmar que no hay secretos reales versionados (usuario real, passwords, service_role key).
- Lint de `.env.example` luego del placeholder: `G360_INTRANET_USER=tu_usuario_intranet` (placeholder, no usuario real), `G360_INTRANET_PASS=tu_password` (placeholder). `SUPABASE_URL`/`SUPABASE_KEY` documentadas como legacy si el código nunca las lee.
- Sacar input de `service_role key` del frontend (`frontend/index.html` líneas 671, 912). Solo `supabase_url` y `supabase_key` (anon) en UI.
- Rotar/regenerar la service_role key en Supabase como acción post-implementación: sacar el input del frontend no deshace que la key ya fue expuesta en la UI del navegador. Las RLS/REVOKE de §14 mitigan, pero la rotación es la práctica correcta.

---