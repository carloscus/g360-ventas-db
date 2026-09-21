"""Genera fixtures XLSX para probar la deteccion de SKU numerico vs texto.

- sku_text.xlsx:  SKU como TEXTO  -> ERP correcto (ceros preservados)
- sku_numeric.xlsx: SKU como NUMERO -> re-guardado en Excel (ceros perdidos)
"""
import os
import openpyxl

fixtures = os.path.join(os.path.dirname(__file__), 'fixtures')
os.makedirs(fixtures, exist_ok=True)

headers = ['ANHO', 'MES', 'ID_ARTICULO', 'NOM_ARTICULO', 'CANTIDAD', 'SOLES']

# ERP correcto: SKU como texto con ceros
wb = openpyxl.Workbook()
ws = wb.active
ws.append(headers)
ws.append([2025, '10-OCTUBRE', '011019', 'N SEMIDEPORTIVA FUTBOL', 100, 500.0])
ws.append([2025, '10-OCTUBRE', '011028', 'N DEPORTIVA VOLEY', 50, 300.0])
ws.append([2025, '10-OCTUBRE', '78339', 'ARCHIVADOR VINIFAN', 4800, 13824.0])
wb.save(os.path.join(fixtures, 'sku_text.xlsx'))

# Re-guardado en Excel: SKU como numero (ceros perdidos)
wb = openpyxl.Workbook()
ws = wb.active
ws.append(headers)
ws.append([2025, '10-OCTUBRE', 11019, 'N SEMIDEPORTIVA FUTBOL', 100, 500.0])
ws.append([2025, '10-OCTUBRE', 11028, 'N DEPORTIVA VOLEY', 50, 300.0])
ws.append([2025, '10-OCTUBRE', 78339, 'ARCHIVADOR VINIFAN', 4800, 13824.0])
wb.save(os.path.join(fixtures, 'sku_numeric.xlsx'))

print('Fixtures creados en', fixtures)
for f in ['sku_text.xlsx', 'sku_numeric.xlsx']:
    print(' ', f)
