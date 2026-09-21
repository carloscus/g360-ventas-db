# API de Análisis Comparativos — g360-ventas-db

Base URL: `https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1`
Auth: Header `apikey` + `Authorization: Bearer {ANON_KEY}`

## Vistas disponibles

### 1. vw_comparativo_ventas

Ventas por artículo/mes con variación interanual y mensual (%).

**Columnas:**

| Campo | Tipo | Descripción |
|-------|------|-------------|
| `id_linea` | text | Código de línea |
| `id_articulo` | text | Código de artículo (SKU) |
| `mes_ref` | text | Mes de referencia (YYYY-MM) |
| `cantidad` | integer | Unidades vendidas en el mes |
| `soles` | numeric | Monto en soles |
| `devuelto` | integer | Unidades devueltas |
| `soles_netos` | numeric | Soles netos (ventas - devoluciones) |
| `cantidad_anio_ant` | integer | Unidades vendidas en el mismo mes del año anterior |
| `soles_anio_ant` | numeric | Monto del año anterior |
| `var_cantidad_pct` | numeric | Variación % interanual de cantidad |
| `var_soles_pct` | numeric | Variación % interanual de soles |
| `cantidad_mes_ant` | integer | Unidades vendidas en el mes anterior |
| `soles_mes_ant` | numeric | Monto del mes anterior |
| `var_cantidad_mom_pct` | numeric | Variación % mensual (month-over-month) de cantidad |
| `var_soles_mom_pct` | numeric | Variación % mensual de soles |

**Ejemplo:** Comparativo de un SKU

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_comparativo_ventas?select=*&id_articulo=eq.02211&order=mes_ref.desc&limit=12" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

Response 200:
```json
[
  {
    "id_linea": "0102",
    "id_articulo": "02211",
    "mes_ref": "2026-08",
    "cantidad": 15000,
    "soles": 53622.00,
    "devuelto": 0,
    "soles_netos": 53622.00,
    "cantidad_anio_ant": 12000,
    "soles_anio_ant": 42897.60,
    "var_cantidad_pct": 25.00,
    "var_soles_pct": 25.00,
    "cantidad_mes_ant": 14000,
    "soles_mes_ant": 50047.20,
    "var_cantidad_mom_pct": 7.14,
    "var_soles_mom_pct": 7.14
  }
]
```

### 2. vw_comparativo_linea

Ventas por línea/mes con variación interanual y mensual (%).

**Misma estructura que vw_comparativo_ventas pero agrupado por línea.**

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_comparativo_linea?select=*&id_linea=eq.01&order=mes_ref.desc&limit=12" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 3. vw_historial_venta_cliente

Historial de ventas por cliente+SKU con precio anterior.

**Columnas:**

| Campo | Tipo | Descripción |
|-------|------|-------------|
| `id_cliente` | text | Código de cliente |
| `nom_cliente` | text | Razón social |
| `id_vendedor` | text | Código de vendedor |
| `nom_vendedor` | text | Nombre del vendedor |
| `id_articulo` | text | SKU |
| `nom_articulo` | text | Nombre del producto |
| `id_linea` | text | Código de línea |
| `nom_linea` | text | Nombre de línea |
| `folio_unico` | text | Folio único del documento |
| `tpo_doc` | text | Tipo de documento |
| `serie_doc` | text | Serie |
| `nro_doc` | text | Número |
| `fecha_orig` | text | Fecha original |
| `mes_ref` | text | Mes de referencia |
| `tipo_operacion` | text | venta/devolucion/ajuste_valor/nota_debito |
| `cantidad` | integer | Unidades |
| `soles` | numeric | Monto en soles |
| `precio_unitario` | numeric | Precio unitario de esta venta |
| `precio_anterior` | numeric | Precio de la venta anterior del mismo cliente+SKU |
| `fecha_anterior` | text | Fecha de la venta anterior |
| `precio_anterior2` | numeric | Precio de la penúltima venta |

**Ejemplo:** Histórico de precios de un cliente+SKU

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_historial_venta_cliente?select=*&id_cliente=eq.00068414&id_articulo=eq.02211&order=fecha_orig.desc&limit=20" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 4. vw_historial_articulo

Histórico de artículo: vendido, devuelto, precio, cambios de nombre/línea.

**Columnas:**

| Campo | Tipo | Descripción |
|-------|------|-------------|
| `id_articulo` | text | SKU |
| `original_sku` | text | SKU original |
| `nom_articulo` | text | Nombre del producto |
| `id_linea` | text | Código de línea |
| `nom_linea` | text | Nombre de línea |
| `mes_ref` | text | Mes de referencia |
| `vendido` | integer | Unidades vendidas |
| `devuelto` | integer | Unidades devueltas |
| `cantidad_neta` | integer | Cantidad neta (vendido - devuelto) |
| `soles_vendidos` | numeric | Soles vendidos |
| `soles_netos` | numeric | Soles netos |
| `p_u_venta` | numeric | Precio unitario de venta |
| `p_u_neto` | numeric | Precio unitario neto |
| `n_lineas` | integer | Número de líneas de venta |
| `n_clientes` | integer | Número de clientes distintos |
| `p_u_venta_anterior` | numeric | Precio unitario del mes anterior |
| `p_u_neto_anterior` | numeric | Precio neto del mes anterior |
| `nom_articulo_anterior` | text | Nombre del mes anterior |
| `id_linea_anterior` | text | Línea del mes anterior |
| `mes_anterior` | text | Mes anterior |
| `cambio_nombre` | boolean | True si el nombre cambió |
| `cambio_linea` | boolean | True si la línea cambió |
| `variacion_precio_pct` | numeric | Variación % de precio |

**Ejemplo:** Histórico de un SKU

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_historial_articulo?id_articulo=eq.02211&order=mes_ref.desc&limit=12" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 5. vw_radar_recompra

Cadencia de recompra por cliente+SKU, días silencio, estado oportunidad.

**Columnas:**

| Campo | Tipo | Descripción |
|-------|------|-------------|
| `id_cliente` | text | Código de cliente |
| `nom_cliente` | text | Razón social |
| `id_articulo` | text | SKU |
| `nom_articulo` | text | Nombre del producto |
| `id_linea` | text | Código de línea |
| `nom_linea` | text | Nombre de línea |
| `n_compras` | integer | Número total de compras |
| `ultima_compra` | text | Fecha de la última compra |
| `dias_cadencia` | integer | Días promedio entre compras |
| `precio_promedio` | numeric | Precio promedio de compra |
| `und_por_dia` | numeric | Unidades promedio por día |
| `dias_silencio` | integer | Días desde la última compra |
| `cadencia_efectiva` | integer | Cadencia efectiva (días) |
| `estado_oportunidad` | text | DENTRO_PERIOD / RETRASADO / SIN_MOVIMIENTO |

**Ejemplo:** Radar de recompra de un cliente

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_radar_recompra?select=*&id_cliente=eq.00068414&order=dias_silencio.desc&limit=20" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### 6. mv_venta_linea_sku_mes

Materialized view: venta por línea+SKU+mes (pre-calculada).

**Columnas:**

| Campo | Tipo | Descripción |
|-------|------|-------------|
| `id_linea` | text | Código de línea |
| `id_articulo` | text | SKU |
| `mes_ref` | text | Mes de referencia |
| `cantidad` | integer | Unidades vendidas |
| `soles` | numeric | Monto en soles |
| `devuelto` | integer | Unidades devueltas |
| `soles_netos` | numeric | Soles netos |

```bash
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/mv_venta_linea_sku_mes?select=*&id_linea=eq.01&order=mes_ref.desc&limit=12" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

---

## Filtros comunes

### Por rango de fechas

```bash
&mes_ref=gte.2025-01&mes_ref=lte.2025-12
```

### Por línea específica

```bash
&id_linea=eq.01
```

### Ordenar por variación (los que más crecieron)

```bash
&order=var_soles_pct.desc
```

### Artículos sin movimiento (oportunidades)

```bash
&estado_oportunidad=eq.SIN_MOVIMIENTO&order=dias_silencio.desc
```

---

## Uso típico

### Dashboard de variación mensual

```bash
# Top 10 artículos con mayor caída mensual
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_comparativo_ventas?select=id_articulo,mes_ref,var_soles_mom_pct&var_soles_mom_pct=lt.0&order=var_soles_mom_pct.asc&limit=10" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### Alertas de recompra

```bash
# Clientes con >30 días sin comprar pero con historial activo
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_radar_recompra?select=*&dias_silencio=gte.30&n_compras=gte.3&order=dias_silencio.desc&limit=50" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```

### Histórico de precios para negociación

```bash
# Ver evolución de precios de un cliente+SKU
curl "https://tqdmoytaucnfrpaklprc.supabase.co/rest/v1/vw_historial_venta_cliente?select=fecha_orig,cantidad,soles,precio_unitario,precio_anterior&id_cliente=eq.00068414&id_articulo=eq.02211&order=fecha_orig.desc" \
  -H "apikey: {KEY}" -H "Authorization: Bearer {KEY}"
```
