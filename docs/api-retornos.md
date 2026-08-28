# API de Devoluciones Fisicas — g360-ventas-db

Base URL: `https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1`
Auth: Header `apikey` + `Authorization: Bearer {ANON_KEY}`

## Reglas de Negocio

1. SKU valido → debe existir al menos 1 registro en ventas
2. Saldo suficiente → vendido - devuelto >= cantidad_solicitada
3. Consumo LIFO → facturas ordenadas por fecha_orig DESC
4. Precio neto → solo cuando SUM(abs(cantidad_fae) de NCs totales) >= cantidad_vendida
5. NC parciales → informativas, NO afectan precio
6. Periodo 3 años → alerta, no bloqueo
7. Moneda → todos en Soles

## Endpoints

### 1. Validar SKU

```
GET /retornos/validar-sku?id_articulo={sku}
```

Response 200:
```json
{
  "existe": true,
  "nom_articulo": "FORRO N VINIFAN A4 CRISTAL 26",
  "id_linea": "0102",
  "vendido_total": 50000,
  "devuelto_total": 0
}
```

Response 404:
```json
{"existe": false}
```

### 2. Saldo global por cliente+SKU

```
GET /retornos/saldo?id_cliente={id}&id_articulo={sku}
```

Response 200:
```json
{
  "id_cliente": "00068414",
  "nom_cliente": "PRIMAVERA DISTRIBUIDORES S.A.C.",
  "id_articulo": "02211",
  "nom_articulo": "FORRO N VINIFAN A4 CRISTAL 26",
  "total_vendido": 50000,
  "total_devuelto": 0,
  "saldo_disponible": 50000,
  "moneda": "Soles",
  "primeraventa": "2025-11-16",
  "ultimaventa": "2025-11-16"
}
```

### 3. Facturas disponibles

```
GET /retornos/facturas?id_cliente={id}&id_articulo={sku}&limit=50
```

Response 200:
```json
[
  {
    "folio_unico": "F01/204/44115",
    "fecha_orig": "2025-11-16",
    "cantidad_vendida": 50000,
    "devuelto": 0,
    "saldo_disponible": 50000,
    "precio_unitario": 3.5748,
    "precio_para_devolucion": 3.4321,
    "estado_periodo": "DENTRO_PERIOD",
    "nc_totales": [{"nc_folio": "NCR/215/29154", "total_fae": 50000, "total_monto": 7137.50, "descuento_unit": 0.1427}],
    "nc_parciales": []
  }
]
```

### 4. Calcular consumo LIFO

```
POST /retornos/calcular
Content-Type: application/json

{
  "id_cliente": "00068414",
  "id_articulo": "02211",
  "cantidad_solicitada": 500
}
```

Response 200:
```json
{
  "cantidad_solicitada": 500,
  "cantidad_asignada": 500,
  "total_soles": 1716.05,
  "alertas": [],
  "breakdown": [
    {
      "folio_unico": "F01/204/44115",
      "fecha_orig": "2025-11-16",
      "cantidad_asignada": 500,
      "saldo_original": 50000,
      "saldo_despues": 49500,
      "precio_unitario": 3.5748,
      "precio_para_devolucion": 3.4321,
      "subtotal": 1716.05,
      "estado_periodo": "DENTRO_PERIOD",
      "nc_totales": [{"nc_folio": "NCR/215/29154", "total_fae": 50000, "descuento_unit": 0.1427}],
      "nc_parciales": []
    }
  ]
}
```

Response 400 (saldo insuficiente):
```json
{
  "error": "Saldo insuficiente",
  "detalle": "Solicitado: 1000u, Disponible: 500u",
  "saldo_disponible": 500
}
```

## Ejemplo real: Primavera + 02211

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/retornos/validar-sku?id_articulo=02211" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"

curl -X POST "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/retornos/calcular" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}" \
  -H "Content-Type: application/json" \
  -d '{"id_cliente":"00068414","id_articulo":"02211","cantidad_solicitada":500}'
```

Resultado esperado:
- 1 factura: F01/204/44115 (50,000u @ S/3.5748)
- 1 NC total: NCR/215/29154 (50,000u, S/7,137.50 → S/0.1427/u)
- Precio neto: S/3.5748 - S/0.1427 = S/3.4321/u
- Total: 500 × 3.4321 = S/1,716.05
