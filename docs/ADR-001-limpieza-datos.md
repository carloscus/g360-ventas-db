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
