# deploy_checklist.md
# Checklist para deploy 100% seguro en producción

> Actualizado: nov-2026 (config DPAPI, contrato v4, retención 1277).

## Pre-Deploy
- [ ] Credenciales intranet configuradas (wizard de primer inicio o `config.enc` DPAPI)
- [ ] Credenciales Supabase configuradas (URL + anon key + service role key)
- [ ] Allowlist de líneas configurada (Config Supabase → chips, 24 líneas)
- [ ] Retención Supabase correcta (`supabase_retention_days = 1277` = 3.5 años)
- [ ] Frontend sincronizado: `cd frontend && npm run build` (copia a `dist/`)
- [ ] Build: `cargo tauri build` → `dist/G360 DB Ventas_*.exe`

## Post-Deploy (verificar)
- [ ] BD creada en: `%APPDATA%\g360-db-ventas\data\historial.db`
- [ ] Tabla `ventas` existe (53 columnas, contrato v4)
- [ ] `g360_meta` con `contracto_version=4`, `dim_schema_version=3`
- [ ] Tablas: `mes_checksums`, `day_checksums`, `dim_*`
- [ ] Índices creados (ver `schema.rs` CREATE_INDEXES_SQL)
- [ ] Stats cache actualizada
- [ ] UI: tarjeta Principal ✅ OK, sin alertas críticas

## Primera Captura
- [ ] Rango definido (ej: 2010-01 a hoy) o captura completa ya realizada
- [ ] Retry automático activo (backoff ante `database is locked`)
- [ ] Lock cleanup si hay captura anterior (`capture.lock` huérfano)

## Upload a Supabase
- [ ] `last_supabase_sync = null` (full sync) la primera vez
- [ ] Ventana de retención correcta (1277 días)
- [ ] Filtrado por allowlist activo (`with_line_filter`)
- [ ] Reconciliación mes por mes: `cargo run --bin upload` o botón UI
- [ ] Verificar: `SELECT * FROM vw_ventas_storage;` → `pct_of_500mb` < 80%

## Mantenimiento periódico
- [ ] VACUUM semanal en Supabase: `supabase/ACTIVATE_pg_cron_vacuum.sql` (pg_cron)
- [ ] Rotación local automática: `backup_keep=7`, `cartucho_keep=3` (config)
- [ ] Recordatorio de backup: `backup_reminder_days` (default 7)
- [ ] Tarea programada Windows (opcional): Config → Actualización automática 14:00
