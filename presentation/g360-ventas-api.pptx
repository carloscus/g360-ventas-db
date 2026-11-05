# g360-ventas-db — Presentación (10 diapositivas)

> Outline para generar el PPTX final. Actualizado: Noviembre 2026.

## Diapositiva 1: Título
- Título: **g360-ventas-db** — Motor de datos de ventas CIPSA
- Subtítulo: Captura ERP → SQLite local → Cartucho offline + Supabase online
- Logo G360
- Fecha: Noviembre 2026 · Contrato v4

## Diapositiva 2: El problema
- Clientes ERP dependen de copias manuales (SMB/USB) de `historial.db`
- 2.8 millones de filas (2010 → 2026), 2.3 GB
- Sin control de frescura: datos desactualizados
- Sin verificación de integridad: imposible saber si la copia es válida
- Procesos ineficientes y propensos a errores

## Diapositiva 3: La solución — doble canal
- **Cartucho offline**: `.db` + `.manifest.json` (SHA256) para clientes sin red
- **Supabase online**: PostgREST + vistas para CRM y dashboards
- **Fuente de verdad**: SQLite local (raw-first, todas las líneas del ERP)
- **Filtro de líneas**: allowlist de 24 líneas solo al subir a Supabase
- **Verificación**: contracto_version=4, checksums por mes y por día

## Diapositiva 4: Arquitectura
- **Productor**: Tauri/Rust → captura intranet (login headless) → SQLite (WAL)
- **Normalización**: IDs (cliente zfill8, línea/vendedor sin `01`), fechas ISO, NC/ND
- **Distribución**: Cartucho (VACUUM INTO + manifest) / Supabase (batches de 500)
- **Consumidores**: Apps offline (cartucho) + apps online (PostgREST)
- **Diagrama**: [intranet → captura → SQLite → (cartucho | Supabase) → apps]

## Diapositiva 5: Contrato de datos v4
- `CONTRACTO_VERSION=4` · `DIM_SCHEMA_VERSION=3` · `NORMALIZACION_VERSION=1`
- Tabla `ventas`: 53 columnas (★20 esenciales para Supabase)
- Manifest del cartucho: sha256, allowed_lines, retention_days, day/month checksums
- Clasificación: `NCR+cant<0`→devolución, `NCR/NDB`→ajuste_valor, `F01/BDI`→venta
- Doc completo: `docs/CONTRATO.md` (checklist de integración incluido)

## Diapositiva 6: Seguridad y acceso
- **Cartucho**: copia de archivo + SHA256, sin red
- **Supabase**: anon key (solo SELECT) / service role (escritura backend)
- **RLS**: políticas por vista, `security_invoker=true`
- **Credenciales locales**: DPAPI Windows (`config.enc`, solo esa PC/usuario)
- **Wizard**: primer inicio en PC nueva pide sus propias credenciales

## Diapositiva 7: Calidad y pruebas
- 50 pruebas automatizadas (todas verdes)
- Checksums por mes y por día (day_checksums en manifest)
- Retry automático ante `database is locked` (backoff 2s/4s/8s)
- Alertas estructuradas (Critical/Warning/Info + acción sugerida)
- Validación de fechas y SKU numérico en import

## Diapositiva 8: Operación diaria
- **Inicio rápido**: captura desde (última fecha − 1 día) → cubre docs tardíos
- **Pipeline diario**: captura → verifica integridad → checksums → sube Supabase
- **Reportes XLSX**: consolidado por mes para análisis y auditoría
- **Monitoreo**: 3 tarjetas de estado (Principal / Supabase / Cartucho)
- **Supabase**: retención 3.5 años (1277 días), monitor `pct_of_500mb`

## Diapositiva 9: Lecciones aprendidas
1. **Purga parcial por `max-rows`**: PostgREST limita GET a 1000 filas; un
   `break` con `ids.len() < BATCH` dejaba filas viejas → duplicados ~2x
   (bug corregido nov-2026)
2. **NDB ≠ `nota_debito`**: las notas de débito son `ajuste_valor`; las vistas
   no las reconocían (corregido)
3. **IDs con formato**: `id_cliente` llega corto (`56101`) y se guarda con
   `zfill(8)` (`00056101`); líneas/vendedores traen prefijo `01`
4. **Buffer −1 día**: el ERP postea con retraso; capturar solo "hoy" pierde docs

## Diapositiva 10: Recomendación y próximos pasos
- **Validado**: captura completa 2010-2026 (203 meses), cartucho v4, Supabase
- **Próximos pasos**:
  1. Adaptador Python en ERP (consumo vía API/cartucho)
  2. Auto-sync diario con Task Scheduler
  3. Refresco de materialized views (pg_cron)
  4. Evaluar Supabase Pro si se necesita >500 MB online
  5. Generar presentación PPTX final desde este outline
