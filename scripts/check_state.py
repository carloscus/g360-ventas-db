import sqlite3
con = sqlite3.connect('file:C:/Users/ccusi/AppData/Roaming/g360-db-ventas/data/historial.db?mode=ro', uri=True)
cur = con.cursor()
print('=== ESTADO POST-LIMPIEZA ===')
print(f'Total filas: {cur.execute("SELECT COUNT(*) FROM ventas").fetchone()[0]:,}')
print(f'Meses: {cur.execute("SELECT COUNT(DISTINCT mes_ref) FROM ventas").fetchone()[0]}')
print(f'RUCs validos: {cur.execute("SELECT COUNT(*) FROM dim_ruc WHERE ruc GLOB \"[0-9]*\"").fetchone()[0]}')
print(f'RUCs multi-cliente: {cur.execute("SELECT COUNT(*) FROM dim_ruc WHERE n_clientes_unicos > 1 AND ruc GLOB \"[0-9]*\"").fetchone()[0]}')
