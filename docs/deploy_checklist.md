# deploy_checklist.md
# Checklist para deploy 100% seguro en producción

## Pre-Deploy
- [ ] Credenciales intranet configuradas (.env o config.json)
- [ ] Credenciales Supabase configuradas
- [ ] allowed_lines configurado (config.json)
- [ ] Retención configurada (supabase_retention_days = 1277)

## Post-Deploy (verificar)
- [ ] BD creada en: %APPDATA%\g360-db-ventas\data\historial.db
- [ ] Tabla ventas existe con estructura correcta
- [ ] Tabla dim_ruc existe
- [ ] Tabla mes_checksums existe
- [ ] Índices creados (especialmente idx_venta_serie_nro)
- [ ] Sin RUCs problemáticos (S/N, 00, etc.)
- [ ] Stats cache actualizada

## Primera Captura
- [ ] Rango definido (ej: 2010-01 a hoy)
- [ ] Workers = 3
- [ ] Lock cleanup si hay captura anterior

## Upload
- [ ] last_supabase_sync = null (full sync)
- [ ] Ventana de retención correcta
- [ ] Filtrado por allowlist activo

## Validación Post-Upload
- [ ] Conteo filas locales == remotas (ventana)
- [ ] Checksums mensuales calculados
- [ ] Sin errores en sync_log
