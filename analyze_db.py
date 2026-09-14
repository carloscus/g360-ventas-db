import sqlite3, os, sys
sys.stdout.reconfigure(encoding='utf-8')
db = r'C:\Users\ccusi\AppData\Roaming\g360-db-ventas\data\historial.db'
con = sqlite3.connect(db)
c = con.cursor()
months_db = sorted([r[0] for r in c.execute('SELECT DISTINCT mes_ref FROM ventas ORDER BY 1')])
raw = r'C:\Users\ccusi\AppData\Roaming\g360-db-ventas\data\raw'
csvs = set()
for f in os.listdir(raw):
    if f.startswith('ventas_') and f.endswith('.csv') and '-p' not in f and len(f) == 18:
        csvs.add(f.replace('ventas_','').replace('.csv',''))
print('Meses en BD:', len(months_db), months_db[0], '->', months_db[-1])
print('CSVs mensuales en raw:', len(csvs), min(csvs), '->', max(csvs))
missing_csv = sorted(csvs - set(months_db))
missing_db = sorted(set(months_db) - csvs)
print()
print('CSV sin BD (necesitan re-parse):', len(missing_csv), missing_csv[:5])
print('BD sin CSV (ya normalizados):', len(missing_db), missing_db[:5])
cnt_v = c.execute("SELECT COUNT(*) FROM ventas WHERE id_vendedor GLOB '01*'").fetchone()[0]
cnt_l = c.execute("SELECT COUNT(*) FROM ventas WHERE id_linea GLOB '01*'").fetchone()[0]
print()
print('id_vendedor con prefijo 01:', cnt_v, 'filas')
print('id_linea con prefijo 01:', cnt_l, 'filas')
cols = [r[1] for r in c.execute('PRAGMA table_info(ventas)').fetchall()]
for name in ['id_ubigeo','canal_distribucion','nom_condicion_pago','estado_linea','division','fec_cargo','id_guia']:
    print(name, 'EXISTS' if name in cols else 'NO EXISTS')
con.close()
