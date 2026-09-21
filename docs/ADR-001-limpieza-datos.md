# ADR-001: Evaluación del plan "staging → transformación → Postgres" sobre el pipeline existente

**Fecha**: 2026-09-02 · **Contexto**: propuesta de plan de limpieza/migración
(staging TEXT → transformaciones → tabla final Postgres) evaluada contra el
pipeline Rust actual de g360-ventas-db.

## Decisión

NO migrar a un flujo staging→Postgres. El pipeline existente ya implementa
el plan con mayor trazabilidad. Se adoptan solo 3 acciones puntuales (ver abajo).

## Mapeo plan-propuesto → pipeline-existente

| Punto del plan | Estado | Implementación |
|---|---|---|
| 1. Original intocable | ✅ Mejorado | `raw/*.csv` intocables; `historial.db` reconstruible con "⚡ Reprocesar raw" |
| 2. Staging TEXT | ✅ Equivalente | El CSV crudo es el staging; el parser Rust es la transformación auditada |
| 3. Validar estructura/duplicados | ✅ | Parser + dedup por (folio_unico, id_articulo) + verify_integrity |
| 4. Códigos TEXT sin tocar ceros | ✅ | `clean_id()` solo limpia BOM/NaN/`.0`; SKU/cliente/ubigeo/grupo/tipo/familia intactos |
| 5. Reglas 0101→01, 01177→177 | ⚠️ RECHAZADA | Ver abajo (hallazgos 2 y 3) |
| 6. Mes texto→entero | ✅ | `mes` derivado de `mes_ref` (backfill aplicado) |
| 7. Fechas→DATE, blancos→NULL | ✅ | `parse_date` → NaiveDate; vacío = default (no fecha falsa) |
| 8. Documentos→TEXT | ✅ | tpo/serie/nro/ord_compra/pedido como TEXT |
| 9. Montos NUMERIC | ✅ | `parse_f64_ctx` con heurística miles/decimal validada; precio a 4 dec (no 5) |
| 10. Tabla final | ✅ (44 cols) | + campos derivados que el plan no contempla: tipo_operacion, factura_ref, folio_unico |
| 11. Validar antes de reemplazar | ✅ Mejorado | verify_upload_result (conteo enviado vs recibido), verify_integrity |
| 12. Validar totales | ✅ Extendido | checksums mensuales ahora incluyen SUM(cantidad) además de filas/soles/dólares |
| 13. Pruebas de negocio | ✅ | Vistas cockpit: vw_historial_venta_cliente, vw_radar_recompra, vw_facturas_disponibles |

## Hallazgos con datos reales (diagnóstico ejecutado 2026-09-02)

### H1: Regla ID_LINEA "quitar prefijo 01" rompería el filtro de líneas
24 líneas, TODAS empiezan con "01", cero colisiones al quitar el prefijo.
PERO `is_allowed_line()` compara los ÚLTIMOS 2 caracteres contra allowed_lines.
Normalizar sin tocar config → un "Reprocesar raw" eliminaría:
- `0181`: 2 filas (S/ -1,252.08)
- `0199`: 1,928 filas (S/ 75,367.32)

**Acción adoptada**: NO normalizar ID_LINEA (4 chars consistentes). Agregar
`81` y `99` a allowed_lines (default + config.json) para que el reparse
conservemos esas filas.

### H2: Regla ID_VENDEDOR "01177→177" innecesaria
56 vendedores, TODOS de 5 chars con prefijo "01" (`01009`..`01999`, `01A02`,
`01PE1`, `01T01`), cero colisiones. Ya es un dominio consistente. No tocar.

### H3: DOC_CLIENTE "duplicado" no es duplicado
Los 6,366 RUCs normales mapean 1→1 con id_cliente. Los únicos casos anómalos:
- `doc='00'`: 35 filas, 3 clientes, S/ 13.88 (ERP sin RUC registrado)
- `doc='74966851.'`: 16 filas, 2 clientes, S/ 35.88 — ambos "NO USAR-DIAZ
  QUIROZ" (clientes dados de baja en el ERP)

**Acción adoptada**: NO limpiar (sería falsificar la fuente; son marcadores
del propio ERP). El segundo DOC_CLIENTE del encabezado TXT no se persiste
porque en el modelo venta-ítem es redundante con id_cliente.

### H4: Campos del plan NO adoptados en el modelo
`canal_distribucion`, `division`, `id_guia`, `ord_compra`, `fec_cargo`:
confirmado con el usuario que NO son necesarios. El parser los descarta
en la carga (quedan en el CSV crudo si algún día se necesitan).

## Acciones ejecutadas

1. `default_allowed_lines()`: agregados "81" y "99" (config.rs)
2. `config.json` local: allowed_lines con 81/99
3. `mes_checksums.total_cantidad` (schema + calculate_monthly_checksums con
   SUM(cantidad) en el hash de 4 componentes)
4. Build + instalación de la app

## Consecuencias

- Un futuro "Reprocesar raw" ya NO perderá las líneas 0181/0199
- Los checksums detectan alteraciones de cantidades (no solo filas/montos)
- Cualquier limpieza futura de los 2 casos DOC_CLIENTE anómalos queda
  documentada como decisión consciente (hoy: preservar)

---

## Addendum 2026-09-02: renombres de razón social y NCs huérfanas

### Renombres (casos 47138 y 17953/TIENDAS APOLO)

Verificado contra CSVs crudos y la NCR 215-7171 (2018): el reporte de
estadísticas del ERP **estampa el nombre actual del master al exportar**, no el
nombre histórico de la transacción. La NCR de 2018 aparece con
"TIENDAS APOLO S.A.C." (nombre 2026) aunque el comprobante físico lleve la
razón social de 2018.

**Consecuencia**: los renombres ocurridos antes de la carga masiva (2026) son
irrecuperables desde esta capa — no es un defecto del pipeline, es una
limitación de la fuente. La clave (id_cliente + RUC) es estable, así que las
agregaciones por cliente nunca se parten. El blindaje implementado
(detección de renombres + vw_dim con nombre más reciente) cubre el único caso
que sí puede aparecer: renombres ENTRE capturas futuras.

### NC/ND huérfanas: 3,894 (0.33% de filas, 5.5% de las NCs)

NCs/NDs cuya factura referenciada NO está en la BD:

| Serie referenciada | NCs | Soles | Causa |
|--------------------|-----|-------|-------|
| 201 | 2,783 | S/ 1.66M | factura pre-2018 o excluida por allowed_lines |
| 1 | 481 | S/ 57K | serie antigua (fuera del rango exportable del intranet) |
| 211 (FEX) | 398 | S/ 726K | factura pre-2018 |
| otras | 232 | — | ídem |

**Impacto en el flujo de devoluciones: ninguno.** La app de retornos parte de
la FACTURA y busca sus NCs (join `factura_ref → serie/nro`); una NC huérfana
apunta a una factura que no existe en la capa de datos, por lo que nunca
participa en un cálculo de saldo. El saldo de cada factura capturada usa solo
las devoluciones que la referencian a ella.

**Regla documentada**: si algún día se exporta el rango pre-2018 (si el
intranet lo permitiera), estas huérfanas se resolverían solas. No requiere
acción.

---

## Addendum 2026-09-15: guard de SKU y propiedad de las alertas de integridad

### Colisiones de ceros en SKU: 82 grupos, TODOS falsos positivos

`LTRIM(id_articulo,'0')` agrupa 82 pares de SKUs que solo comparten dígitos tras
quitar ceros. Verificado contra la BD: **los 82 pares son productos DISTINTOS en
líneas distintas** (0 casos en la misma línea):

| Variante | Línea | Producto | Variante | Línea | Producto |
|---|---|---|---|---|---|
| `011019` | 01 PELOTAS | N SEMIDEPORTIVA FUTBOL | `11019` | 11 METALICA | PERFORADOR VINIFAN |
| `014014` | 01 PELOTAS | PELOTA SALTARINA PAW PATROL | `14014` | 14 OTROS | DESCUENTO JUGUETE |
| `014186` | 01 PELOTAS | … | `14186` | 73 … | … |

Distribución: 56 pares (L01↔L14), 16 (L01↔L73), 8 (L01↔L11), 2 (L01↔L02).

**El cero inicial es significativo**: distingue la línea `01` (pelotas) del
resto. El detector en `writer.rs` es informativo, no marca corrupción.

### Cadena SKU verificada extremo a extremo (sin modificación)

```
ERP/intranet (celda TEXTO "011019")
  → xls_to_csv (calamine Data::String)      → "011019"
  → clean_id() (solo quita BOM y ".0")       → "011019"
  → SQLite id_articulo TEXT                  → "011019"
  → Supabase                                 → "011019"
```

Evidencia: `ventas_2025-10.csv` tiene 8,558/19,990 filas con cero inicial, sin
un solo artefacto `.0`. `clean_id()` NO hace zero-fill ni zero-strip.

### Riesgo real y guard implementado

Único punto de falla: **importación manual por XLS**. `xls.rs` convierte celdas
`Data::Float` con `{:.0}`, así que si Excel tipó la columna SKU como número,
`011019` llega como `11019` — cero perdido en origen, irrecuperable.

**Guard implementado**:

1. `processor::xls::inspect_sku_column()` — detecta celdas numéricas en la
   columna `ID_ARTICULO` sin convertir ni insertar.
2. `xls_to_csv()` — registra `warn!` durante la conversión (cubre la captura
   automática como defensa en profundidad, sin bloquear el pipeline).
3. `import_manual_day` / `import_manual_month` — **bloquean** con mensaje
   accionable ("re-exporta sin re-guardar en Excel").
4. `preview_import()` — muestra el aviso antes de confirmar.
5. Tests en `tests/sku_column_guard.rs` + fixtures `tests/fixtures/`.

### Propiedad de las alertas (decisión de alcance)

Las alertas restantes de `detectar_entidades_duplicadas` son **propias de la
fuente (ERP), no del pipeline**:

| Alerta | Origen | Acción |
|---|---|---|
| 🟡 RUC duplicado (3) | Renombres/fusiones del ERP | ERP; la clave `id_cliente` no se parte |
| 🟡 Nombres repetidos (133) | Split por renombre ERP | ERP; `vw_dim_cliente` muestra el más reciente |
| ℹ️ SKU colisión ceros (82) | Códigos legítimos distintos | Falso positivo, informativo |
| ℹ️ Clientes/SKUs renombrados | Historia del catálogo | Informativo, auditable |

El pipeline preserva la fuente; solo protege contra daño *introducido en la
importación* (el guard de SKU). No "corrige" datos del ERP.

---

## Addendum 2026-09-15: jerarquía ID estable → nombre vigente (dims)

### Decisión

Toda dimensión se keyed por su **ID estable** y expone los atributos
(nombre, RUC, ubicación, catálogo) de la **fila más reciente**, no del `MAX()`
alfabético. Jerarquía adoptada:

```
id_cliente  → doc_cliente (RUC) → nom_cliente ─┐
id_vendedor → nom_vendedor                     ├─ atributo por fila más reciente
id_linea    → nom_linea / grupo / familia      │
id_articulo → original_sku → nom_articulo ─────┘
```

Si el ERP renombra un cliente/SKU/línea/vendedor **sin cambiar el código**,
las dimensiones muestran el nombre vigente. Igual que ya hacían
`vw_dim_cliente`/`vw_dim_articulo` (migración Supabase 20250902000004).

### Estado previo (inconsistencia encontrada)

| Objeto | Semántica previa | Correcto |
|---|---|---|
| `vw_dim_cliente` (vista) | `ROW_NUMBER` más reciente | ✅ |
| `vw_dim_articulo` (vista) | `ROW_NUMBER` más reciente | ✅ |
| `vw_dim_linea` (vista) | `MAX(nom_linea)` | ❌ → corregido |
| `dim_cliente` (tabla) | `MAX(nom_cliente)` | ❌ → corregido |
| `dim_vendedor` (tabla) | `MAX(nom_vendedor)` | ❌ → corregido |
| `dim_articulo` (tabla) | `MAX(nom_articulo)` | ❌ → corregido |
| `dim_linea` (tabla) | `MAX(nom_linea)` | ❌ → corregido |

**Datos actuales: 0 renombres** en las 4 entidades (el ERP estampa el nombre
actual al exportar). El cambio es *forward-proof*: no altera nada hoy, pero
protege ante renombres futuros entre capturas.

### Bug adicional corregido

`refresh_dim_tables` retornaba temprano si `dim_cliente` ya tenía filas. Eso
hacía que **dedup, migraciones de id_linea/id_vendedor y export** NO refrescaran
las dims (quedaban desactualizadas). Ahora:
- `refresh_dim_tables(pool)` → **siempre** reconstruye.
- `ensure_dims_fresh(pool)` → reconstruye solo si vacías o `PRAGMA user_version
  < DIM_SCHEMA_VERSION`; es el guard del arranque caliente (init_pool cada ~2 s).
- `DIM_SCHEMA_VERSION` (schema.rs) versiona la **semántica**; v2 = nombres
  vigentes. BD existentes migran una sola vez (~100 s con 2.8 M filas).

### Vistas: DROP + CREATE

Las vistas de dimensión pasaron de `CREATE VIEW IF NOT EXISTS` a
`DROP VIEW IF EXISTS ...; CREATE VIEW ...`. Sin esto, una definición nueva
**nunca** reemplazaba una vista ya existente (bug real detectado al aplicar
el cambio: `vw_dim_linea` seguía con `MAX()`). Recrear vistas es instantáneo
(son metadatos); el código es ahora la única fuente de verdad.

### Nota de alcance (local)

Las tablas `dim_*` locales solo se consultan para el conteo del endpoint
`get_db_health`. Las dimensiones expuestas a apps downstream son las vistas
`vw_dim_*`. `insert_ventas` no refresca dims (comportamiento previo, no
modificado): las dims se refrescan en arranque/version-bump/export/dedup.
