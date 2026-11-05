#!/usr/bin/env python3
"""
Genera un cartucho ejemplo para desarrollo de apps cliente.
Uso: python scripts/generate_sample_cartucho.py [--output path/to/cartucho.db]
"""

import sqlite3, json, hashlib, os, sys
from datetime import datetime, timezone

COLS = [
    "id_articulo",
    "original_sku",
    "nom_articulo",
    "id_linea",
    "nom_linea",
    "id_grupo",
    "nom_grupo",
    "id_tipo",
    "nom_tipo",
    "id_familia",
    "nom_familia",
    "id_cliente",
    "doc_cliente",
    "nom_cliente",
    "tpo_doc",
    "serie_doc",
    "nro_doc",
    "referencia",
    "moneda",
    "cantidad",
    "soles",
    "dolares",
    "precio_unitario",
    "cantidad_fae",
    "anho",
    "mes",
    "fecha_orig",
    "fecha_ref",
    "fecha_venc",
    "cod_sucursal",
    "nom_sucursal",
    "departamento",
    "provincia",
    "distrito",
    "id_vendedor",
    "nom_vendedor",
    "id_pedido",
    "ord_compra",
    "file_source",
    "mes_ref",
    "capturado_en",
    "tipo_operacion",
    "factura_ref_serie",
    "factura_ref_nro",
    "folio_unico",
    "id_ubigeo",
    "canal_distribucion",
    "nom_condicion_pago",
    "estado_linea",
    "division",
    "fec_cargo",
    "id_guia",
]
NCOL = len(COLS)
PLACEHOLDERS = ",".join(["?"] * NCOL)


def row(
    sku,
    nom,
    linea,
    nl,
    cli,
    ruc,
    ncli,
    tpo,
    serie,
    nro,
    ref,
    moneda,
    cant,
    soles,
    dol,
    pu,
    fae,
    anho,
    mes,
    fecha,
    tipo,
    folio,
    vend,
    nvend,
    dep="",
    prov="",
    dist="",
):
    return [
        sku,
        sku,
        nom,
        linea,
        nl,
        "",
        "",
        "",
        "",
        "",
        "",
        cli,
        ruc,
        ncli,
        tpo,
        serie,
        nro,
        ref,
        moneda,
        cant,
        soles,
        dol,
        pu,
        fae,
        anho,
        mes,
        fecha,
        "",
        "",
        "",
        "",
        dep,
        prov,
        dist,
        vend,
        nvend,
        "",
        "",
        f"ventas_{anho}-{mes:02d}.csv",
        f"{anho}-{mes:02d}",
        "",
        tipo,
        "",
        "",
        folio,
        "",
        "",
        "",
        "",
        "",
        "",
        "",
    ]


VENTAS = [
    row(
        "SKU001",
        "PRODUCTO A",
        "01",
        "LINEA A",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "F01",
        "204",
        "10001",
        "",
        "Soles",
        10,
        150.00,
        0.0,
        15.00,
        10,
        2025,
        10,
        "2025-10-05",
        "venta",
        "F01/204/10001",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU001",
        "PRODUCTO A",
        "01",
        "LINEA A",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "F01",
        "204",
        "10002",
        "",
        "Soles",
        5,
        75.00,
        0.0,
        15.00,
        5,
        2025,
        10,
        "2025-10-12",
        "venta",
        "F01/204/10002",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU002",
        "PRODUCTO B",
        "02",
        "LINEA B",
        "00000002",
        "20987654321",
        "OTRO CLIENTE EIRL",
        "F01",
        "204",
        "10003",
        "",
        "Soles",
        20,
        300.00,
        40.00,
        15.00,
        20,
        2025,
        10,
        "2025-10-15",
        "venta",
        "F01/204/10003",
        "102",
        "VENDEDOR 2",
        "AREQUIPA",
        "AREQUIPA",
        "AREQUIPA",
    ),
    row(
        "SKU003",
        "PRODUCTO C",
        "78",
        "LINEA C",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "F01",
        "204",
        "10004",
        "",
        "Soles",
        3,
        450.00,
        0.0,
        150.00,
        3,
        2025,
        10,
        "2025-10-20",
        "venta",
        "F01/204/10004",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU001",
        "PRODUCTO A",
        "01",
        "LINEA A",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "NCR",
        "204",
        "90001",
        "F01/204-10001",
        "Soles",
        -2,
        -30.00,
        0.0,
        15.00,
        -2,
        2025,
        10,
        "2025-10-25",
        "devolucion",
        "NCR/204/90001",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU001",
        "PRODUCTO A",
        "01",
        "LINEA A",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "F01",
        "204",
        "11001",
        "",
        "Soles",
        8,
        120.00,
        0.0,
        15.00,
        8,
        2025,
        11,
        "2025-11-03",
        "venta",
        "F01/204/11001",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU002",
        "PRODUCTO B",
        "02",
        "LINEA B",
        "00000002",
        "20987654321",
        "OTRO CLIENTE EIRL",
        "F01",
        "204",
        "11002",
        "",
        "Soles",
        15,
        225.00,
        30.00,
        15.00,
        15,
        2025,
        11,
        "2025-11-10",
        "venta",
        "F01/204/11002",
        "102",
        "VENDEDOR 2",
        "AREQUIPA",
        "AREQUIPA",
        "AREQUIPA",
    ),
    row(
        "SKU004",
        "PRODUCTO D",
        "MA",
        "LINEA D",
        "00000003",
        "20555555555",
        "TERCER CLIENTE SA",
        "F01",
        "205",
        "11003",
        "",
        "Soles",
        1,
        1500.00,
        200.00,
        1500.00,
        1,
        2025,
        11,
        "2025-11-18",
        "venta",
        "F01/205/11003",
        "103",
        "VENDEDOR 3",
        "CUSCO",
        "CUSCO",
        "CUSCO",
    ),
    row(
        "SKU002",
        "PRODUCTO B",
        "02",
        "LINEA B",
        "00000002",
        "20987654321",
        "OTRO CLIENTE EIRL",
        "NCR",
        "204",
        "90002",
        "F01/204-11002",
        "Soles",
        -15,
        -225.00,
        -30.00,
        15.00,
        -15,
        2025,
        11,
        "2025-11-28",
        "devolucion",
        "NCR/204/90002",
        "102",
        "VENDEDOR 2",
        "AREQUIPA",
        "AREQUIPA",
        "AREQUIPA",
    ),
    row(
        "SKU001",
        "PRODUCTO A",
        "01",
        "LINEA A",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "F01",
        "204",
        "12001",
        "",
        "Soles",
        12,
        180.00,
        0.0,
        15.00,
        12,
        2025,
        12,
        "2025-12-02",
        "venta",
        "F01/204/12001",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU003",
        "PRODUCTO C",
        "78",
        "LINEA C",
        "00000001",
        "20123456789",
        "CLIENTE DEMO SAC",
        "F01",
        "204",
        "12002",
        "",
        "Soles",
        4,
        600.00,
        80.00,
        150.00,
        4,
        2025,
        12,
        "2025-12-10",
        "venta",
        "F01/204/12002",
        "101",
        "VENDEDOR 1",
        "LIMA",
        "LIMA",
        "LIMA",
    ),
    row(
        "SKU004",
        "PRODUCTO D",
        "MA",
        "LINEA D",
        "00000003",
        "20555555555",
        "TERCER CLIENTE SA",
        "F01",
        "205",
        "12003",
        "",
        "Soles",
        2,
        3000.00,
        400.00,
        1500.00,
        2,
        2025,
        12,
        "2025-12-15",
        "venta",
        "F01/205/12003",
        "103",
        "VENDEDOR 3",
        "CUSCO",
        "CUSCO",
        "CUSCO",
    ),
    row(
        "SKU002",
        "PRODUCTO B",
        "02",
        "LINEA B",
        "00000002",
        "20987654321",
        "OTRO CLIENTE EIRL",
        "F01",
        "204",
        "12004",
        "",
        "Soles",
        10,
        150.00,
        20.00,
        15.00,
        10,
        2025,
        12,
        "2025-12-20",
        "venta",
        "F01/204/12004",
        "102",
        "VENDEDOR 2",
        "AREQUIPA",
        "AREQUIPA",
        "AREQUIPA",
    ),
]

SCHEMA = """
CREATE TABLE IF NOT EXISTS ventas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    id_articulo TEXT NOT NULL, original_sku TEXT, nom_articulo TEXT,
    id_linea TEXT NOT NULL, nom_linea TEXT,
    id_grupo TEXT, nom_grupo TEXT, id_tipo TEXT, nom_tipo TEXT,
    id_familia TEXT, nom_familia TEXT,
    id_cliente TEXT NOT NULL, doc_cliente TEXT, nom_cliente TEXT,
    tpo_doc TEXT NOT NULL, serie_doc TEXT, nro_doc TEXT,
    referencia TEXT, moneda TEXT DEFAULT 'Soles',
    cantidad REAL NOT NULL, soles REAL NOT NULL, dolares REAL, precio_unitario REAL,
    cantidad_fae REAL, anho INTEGER NOT NULL, mes INTEGER NOT NULL,
    fecha_orig TEXT NOT NULL, fecha_ref TEXT, fecha_venc TEXT,
    cod_sucursal TEXT, nom_sucursal TEXT,
    departamento TEXT, provincia TEXT, distrito TEXT,
    id_vendedor TEXT NOT NULL, nom_vendedor TEXT,
    id_pedido TEXT, ord_compra TEXT,
    file_source TEXT, mes_ref TEXT NOT NULL, capturado_en TEXT DEFAULT '',
    tipo_operacion TEXT DEFAULT 'venta',
    factura_ref_serie TEXT, factura_ref_nro TEXT, folio_unico TEXT,
    id_ubigeo TEXT, canal_distribucion TEXT, nom_condicion_pago TEXT,
    estado_linea TEXT, division TEXT, fec_cargo TEXT, id_guia TEXT
);
CREATE INDEX IF NOT EXISTS idx_venta_mes ON ventas(mes_ref);
CREATE INDEX IF NOT EXISTS idx_venta_cliente ON ventas(id_cliente);
CREATE INDEX IF NOT EXISTS idx_venta_sku ON ventas(id_articulo);
CREATE INDEX IF NOT EXISTS idx_venta_fecha ON ventas(fecha_orig);
CREATE INDEX IF NOT EXISTS idx_venta_doc ON ventas(tpo_doc, serie_doc, nro_doc);
CREATE TABLE IF NOT EXISTS g360_meta (key TEXT NOT NULL PRIMARY KEY, value TEXT NOT NULL, actualizado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS mes_checksums (mes_ref TEXT NOT NULL PRIMARY KEY, checksum TEXT NOT NULL, total_filas INTEGER NOT NULL, total_soles REAL NOT NULL, total_cantidad REAL, calculado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS day_checksums (dia TEXT NOT NULL PRIMARY KEY, filas INTEGER NOT NULL, soles REAL NOT NULL, checksum TEXT NOT NULL, calculado_en TEXT);
CREATE TABLE IF NOT EXISTS dim_cliente (id_cliente TEXT PRIMARY KEY, nom_cliente TEXT NOT NULL, doc_cliente TEXT, departamento TEXT, provincia TEXT, distrito TEXT, n_ventas INTEGER DEFAULT 0, total_soles REAL DEFAULT 0, total_dolares REAL DEFAULT 0, ultima_compra TEXT, actualizado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS dim_vendedor (id_vendedor TEXT PRIMARY KEY, nom_vendedor TEXT NOT NULL, n_ventas INTEGER DEFAULT 0, total_soles REAL DEFAULT 0, n_clientes INTEGER DEFAULT 0, ultima_venta TEXT, actualizado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS dim_articulo (id_articulo TEXT PRIMARY KEY, original_sku TEXT, nom_articulo TEXT NOT NULL, id_linea TEXT, nom_linea TEXT, id_grupo TEXT, nom_grupo TEXT, id_tipo TEXT, nom_tipo TEXT, id_familia TEXT, nom_familia TEXT, n_ventas INTEGER DEFAULT 0, total_cantidad REAL DEFAULT 0, total_soles REAL DEFAULT 0, precio_promedio REAL DEFAULT 0, ultima_venta TEXT, actualizado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS dim_documento (folio_unico TEXT PRIMARY KEY, tpo_doc TEXT NOT NULL, serie_doc TEXT NOT NULL, nro_doc TEXT NOT NULL, fecha_orig TEXT NOT NULL, id_cliente TEXT NOT NULL, id_vendedor TEXT, n_lineas INTEGER DEFAULT 0, total_cantidad REAL DEFAULT 0, total_soles REAL DEFAULT 0, total_dolares REAL DEFAULT 0, moneda TEXT DEFAULT 'Soles', actualizado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS dim_linea (id_linea TEXT PRIMARY KEY, nom_linea TEXT, id_grupo TEXT, nom_grupo TEXT, n_ventas INTEGER DEFAULT 0, total_soles REAL DEFAULT 0, n_clientes INTEGER DEFAULT 0, actualizado_en TEXT DEFAULT '');
CREATE TABLE IF NOT EXISTS fact_venta_mes (id_linea TEXT NOT NULL, id_articulo TEXT NOT NULL, mes_ref TEXT NOT NULL, cantidad REAL DEFAULT 0, soles REAL DEFAULT 0, devuelto REAL DEFAULT 0, soles_netos REAL DEFAULT 0, PRIMARY KEY (id_linea, id_articulo, mes_ref));
"""


def generate(output_path):
    if os.path.exists(output_path):
        os.remove(output_path)
    conn = sqlite3.connect(output_path)
    c = conn.cursor()
    c.executescript(SCHEMA)

    now = datetime.now(timezone.utc).isoformat()
    c.executemany(
        "INSERT OR REPLACE INTO g360_meta VALUES (?,?,?)",
        [
            ("contracto_version", "4", now),
            ("dim_schema_version", "3", now),
            ("normalizacion_version", "1", now),
        ],
    )

    sql = f"INSERT INTO ventas ({','.join(COLS)}) VALUES ({PLACEHOLDERS})"
    for v in VENTAS:
        assert len(v) == NCOL, f"Expected {NCOL}, got {len(v)}: {v[:5]}"
        c.execute(sql, v)

    # Checksums mensuales
    c.execute("""INSERT OR REPLACE INTO mes_checksums
        (mes_ref,checksum,total_filas,total_soles,total_cantidad,calculado_en)
        SELECT mes_ref, printf('%08x-%08x-%08x-%08x',COUNT(*),
            CAST(ROUND(SUM(soles)*100) AS INTEGER)&0xFFFFFFFF,
            CAST(ROUND(SUM(COALESCE(dolares,0)*100),0) AS INTEGER)&0xFFFFFFFF,
            CAST(ROUND(SUM(cantidad)*100) AS INTEGER)&0xFFFFFFFF),
            COUNT(*),ROUND(SUM(soles),2),ROUND(SUM(cantidad),4),datetime('now')
        FROM ventas GROUP BY mes_ref""")

    # Checksums diarios
    c.execute("""INSERT INTO day_checksums (dia,filas,soles,checksum,calculado_en)
        SELECT fecha_orig,COUNT(*),ROUND(SUM(soles),2),
            COUNT(*)||':'||ROUND(SUM(soles),2),datetime('now')
        FROM ventas WHERE fecha_orig!='' GROUP BY fecha_orig""")

    # Dimensiones
    c.execute(
        "INSERT OR REPLACE INTO dim_cliente SELECT id_cliente,nom_cliente,doc_cliente,'','','',COUNT(*),SUM(soles),0,MAX(fecha_orig),datetime('now') FROM ventas GROUP BY id_cliente"
    )
    c.execute(
        "INSERT OR REPLACE INTO dim_vendedor SELECT id_vendedor,nom_vendedor,COUNT(*),SUM(soles),COUNT(DISTINCT id_cliente),MAX(fecha_orig),datetime('now') FROM ventas GROUP BY id_vendedor"
    )
    c.execute(
        "INSERT OR REPLACE INTO dim_articulo SELECT id_articulo,original_sku,nom_articulo,id_linea,nom_linea,'','','','','','',COUNT(*),SUM(cantidad),SUM(soles),ROUND(AVG(precio_unitario),4),MAX(fecha_orig),datetime('now') FROM ventas GROUP BY id_articulo"
    )
    c.execute(
        "INSERT OR REPLACE INTO dim_documento SELECT folio_unico,tpo_doc,serie_doc,nro_doc,fecha_orig,id_cliente,id_vendedor,COUNT(*),SUM(cantidad),SUM(soles),0,'Soles',datetime('now') FROM ventas GROUP BY folio_unico"
    )
    c.execute(
        "INSERT OR REPLACE INTO dim_linea SELECT id_linea,nom_linea,'','',COUNT(*),SUM(soles),COUNT(DISTINCT id_cliente),datetime('now') FROM ventas GROUP BY id_linea"
    )
    c.execute(
        "INSERT OR REPLACE INTO fact_venta_mes SELECT id_linea,id_articulo,mes_ref,SUM(CASE WHEN tipo_operacion='venta' THEN cantidad ELSE 0 END),SUM(CASE WHEN tipo_operacion='venta' THEN soles ELSE 0 END),SUM(CASE WHEN tipo_operacion='devolucion' THEN ABS(cantidad) ELSE 0 END),SUM(soles) FROM ventas GROUP BY id_linea,id_articulo,mes_ref"
    )

    conn.commit()
    total = c.execute("SELECT COUNT(*) FROM ventas").fetchone()[0]
    meses = c.execute("SELECT COUNT(DISTINCT mes_ref) FROM ventas").fetchone()[0]
    min_f = c.execute("SELECT MIN(fecha_orig) FROM ventas").fetchone()[0] or ""
    max_f = c.execute("SELECT MAX(fecha_orig) FROM ventas").fetchone()[0] or ""
    total_soles = c.execute("SELECT ROUND(SUM(soles),2) FROM ventas").fetchone()[0] or 0
    conn.close()

    with open(output_path, "rb") as f:
        sha256 = hashlib.sha256(f.read()).hexdigest()

    manifest = {
        "version": 4,
        "contracto_version": "4",
        "dim_schema_version": "3",
        "normalizacion_version": "1",
        "format": "g360-ventas-cartucho-v2",
        "tipo": "ejemplo",
        "generado_en": datetime.now(timezone.utc).isoformat(),
        "generado_por": "generate_sample_cartucho.py/0.1.0",
        "rango": {"desde": min_f, "hasta": max_f},
        "sha256": sha256,
        "quick_check": {
            "ok": True,
            "filas": total,
            "soles": total_soles,
            "meses": meses,
        },
        "espejo_completo": True,
        "contenido": "ejemplo sintetico para desarrollo de apps cliente",
    }
    manifest_path = output_path.replace(".db", ".manifest.json")
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)

    print(f"OK: {output_path}")
    print(
        f"  {os.path.getsize(output_path) / (1024 * 1024):.2f} MB | {total} rows | {meses} months | {min_f} to {max_f}"
    )
    print(f"  SHA256: {sha256[:32]}...")
    print(f"  Manifest: {manifest_path}")


if __name__ == "__main__":
    out = (
        sys.argv[2]
        if len(sys.argv) > 2 and sys.argv[1] == "--output"
        else os.path.join(
            os.path.dirname(__file__), "..", "data", "export", "cartucho_ejemplo.db"
        )
    )
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    generate(os.path.abspath(out))
