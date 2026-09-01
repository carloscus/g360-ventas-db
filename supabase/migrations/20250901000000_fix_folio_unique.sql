-- Migración: Quitar constraint única de folio_unico
-- La deduplicación se maneja a nivel de aplicación (no en BD)
-- para permitir facturas multi-línea (múltiples SKUs por factura)

ALTER TABLE public.ventas DROP CONSTRAINT IF EXISTS uq_ventas_folio_unico;
ALTER TABLE public.ventas DROP CONSTRAINT IF EXISTS uq_ventas_folio_sku;
