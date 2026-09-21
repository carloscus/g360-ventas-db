-- Refrescar vw_radar_recompra para sincronizar con id_linea normalizada.
-- La MV se creo en 20250902 con codigos de 4 chars (0176).
-- Tras la migracion 2026-09-13 en writer.rs, id_linea en ventas esta normalizado
-- a 2 chars (quitando prefijo 01): 0176 -> 76.
-- Este refresh reconstruye la MV con los datos actualizados.

REFRESH MATERIALIZED VIEW CONCURRENTLY public.vw_radar_recompra;
