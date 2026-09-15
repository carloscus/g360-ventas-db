import re
src = open(r'src\db\writer.rs', encoding='utf-8').read()
old = '''    // Mismo RUC (doc_cliente) apuntando a 2+ id_cliente — excluye anomalias ERP conocidas
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT doc_cliente k, COUNT(DISTINCT id_cliente) n
            FROM ventas
            WHERE doc_cliente NOT IN ('', '00') AND LENGTH(doc_cliente) >= 10
            GROUP BY k HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;'''
new = '''    // Mismo RUC (doc_cliente) apuntando a 2+ id_cliente — excluye anomalias ERP conocidas
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT doc_cliente) FROM (
            SELECT doc_cliente, COUNT(DISTINCT id_cliente) as n
            FROM ventas
            WHERE doc_cliente NOT IN ('', '00') AND LENGTH(doc_cliente) >= 10
            GROUP BY doc_cliente HAVING n > 1
        )"
    )
    .fetch_one(pool)
    .await?;'''
assert old in src, "old not found"
src = src.replace(old, new)
open(r'src\db\writer.rs', 'w', encoding='utf-8').write(src)
print('Fixed RUC query')
