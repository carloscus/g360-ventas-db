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
