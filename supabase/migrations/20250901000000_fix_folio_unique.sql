-- Migración: Cambiar constraint única de folio_unico a (folio_unico, id_articulo)
-- Para permitir múltiples líneas por factura (SKU diferente en misma factura)

-- Eliminar constraint antigua
ALTER TABLE public.ventas DROP CONSTRAINT IF EXISTS uq_ventas_folio_unico;

-- Crear constraint nueva compuesta
ALTER TABLE public.ventas ADD CONSTRAINT uq_ventas_folio_sku UNIQUE (folio_unico, id_articulo);

-- Crear índice para upsert
CREATE INDEX IF NOT EXISTS idx_venta_folio_sku ON public.ventas (folio_unico, id_articulo);
