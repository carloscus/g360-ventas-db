# Investigacion BDI sub-captura

## Problema
El split por rango de articulos en download_day_split pierde documentos BDI.

## Hipotesis
El ERP filtra BDI (devoluciones) de forma diferente cuando se especifica un rango de articulos.
Los BDI tienen tpo_doc=BDI y son tratados como venta en el parser.

## Evidencia
- 2026-11-16: intranet 590 filas/64 BDI vs cartucho 250 filas/2 BDI
- El split por articulo (2->4->8 partes) cubre rangos lexicograficos
- Los BDI pueden tener codigos de articulo fuera de los rangos cubiertos

## Fix recomendado
1. Agregar captura separada para BDI sin filtro de articulo
2. O agregar logging de conteo BDI por chunk para detectar perdida
3. Verificar que todos los rangos de articulos cubran el espacio completo

## Estado
Investigacion pendiente - requiere acceso al ERP para validar

## Hallazgos de prueba directa (18/11/2026)

Se probó la URL del ERP sin filtro de articulos (valueArtI= y valueArtF= vacios):
- Fecha: 05/11/2026
- Resultado: El ERP devuelve documentos BDI (se observa ACUMULADO BDI en el grid)
- El grid incluye columnas: TPO_DOC, SERIE_DOC, NRO_DOC, ID_ARTICULO, etc.

### Hipotesis confirmada parcialmente
Cuando se filtra por rango de articulos (valueArtI/valueArtF), el ERP puede no devolver
documentos BDI porque:
1. Los BDI tienen articulos con codigos que caen fuera de los rangos definidos
2. El ERP puede filtrar BDI de forma diferente cuando se especifica rango de articulos

### Rangos actuales en download_day_split
- Scheme 1: vacio..5ZZZZZ, 6..ZZZZZ
- Scheme 2: vacio..3ZZZZZ, 4..6ZZZZZ, 7..9ZZZZZ, :..ZZZZZ
- Scheme 3: 8 buckets cubriendo vacio..ZZZZZ

Los rangos cubren el espacio completo de codigos alfanumericos (0-9, A-Z, :).
Sin embargo, BDI con articulos que tienen codigos especiales o vacios pueden perderse.

### Fix recomendado (actualizado)
1. Agregar captura separada para BDI sin filtro de articulo (valueArtI= y valueArtF= vacios)
2. Contar BDI por bucket en el log para detectar perdida
3. Comparar conteo total de BDI antes/despues del split
