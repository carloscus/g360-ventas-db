/// Verificación de despliegue para producción
/// Garantiza que la BD esté en estado correcto antes de operar

use anyhow::{Result, Context};
use sqlx::SqlitePool;

/// Verifica que una BD recién desplegada esté lista para producción
pub async fn verify_deploy(pool: &SqlitePool) -> Result<Vec<String>> {
    let mut issues = Vec::new();

    // 1. Tablas críticas existen
    let tables = ["ventas", "dim_ruc", "mes_checksums", "sync_log", "audit_log"];
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        ))
        .bind(table)
        .fetch_one(pool)
        .await
        .context(format!("Query falló para tabla {}", table))?;
        if count == 0 {
            issues.push(format!("❌ Tabla crítica faltante: {}", table));
        }
    }

    // 2. Índices críticos existen
    let critical_indexes = [
        "idx_venta_serie_nro",      // Fix crítico: query plan O(n^2)
        "idx_venta_folio_articulo", // Dedup/unique
        "idx_venta_mes",            // Range queries
        "idx_dim_ruc_nombre",       // Búsqueda por nombre
    ];
    for idx in critical_indexes {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=? AND tbl_name='ventas'"
        ))
        .bind(idx)
        .fetch_one(pool)
        .await?;
        if idx.starts_with("idx_dim_") {
            // Para dim_ruc
            let count: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?"
            ))
            .bind(idx)
            .fetch_one(pool)
            .await?;
            if count == 0 {
                issues.push(format!("❌ Índice crítico faltante: {}", idx));
            }
        } else if count == 0 {
            issues.push(format!("❌ Índice crítico faltante: {}", idx));
        }
    }

    // 3. Estructura de ventas (columnas críticas)
    let col_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('ventas')",
    )
    .fetch_one(pool)
    .await?;
    if col_count < 50 {
        issues.push(format!("⚠️ Tabla ventas incompleta: {} columnas (esperado 53+)", col_count));
    }

    // 4. RUCs problemáticos (deben ser 0 tras limpieza)
    let bad_rucs: i64 = sqlx::query_scalar(r#"
        SELECT COUNT(*) FROM (
            SELECT doc_cliente FROM ventas 
            WHERE doc_cliente NOT GLOB '[0-9]*' 
              AND doc_cliente != ''
              AND doc_cliente NOT IN ('S/N', 'N/A', 'PENDIENTE')
            GROUP BY doc_cliente HAVING COUNT(*) > 5
        )
    "#)
    .fetch_one(pool)
    .await?;
    if bad_rucs > 0 {
        issues.push(format!("⚠️ RUCs problemáticos detectados: {}", bad_rucs));
    }

    // 5. Datos mínimos (BD vacía = problema)
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas").fetch_one(pool).await?;
    if row_count == 0 {
        issues.push("ℹ️ BD vacía: necesita captura inicial".to_string());
    }

    // 6. dim_ruc poblado
    let dim_ruc_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dim_ruc").fetch_one(pool).await?;
    if dim_ruc_count == 0 && row_count > 0 {
        issues.push("⚠️ dim_ruc vacío pero ventas tiene datos: ejecutar refresh_dim_ruc".to_string());
    }

    if issues.is_empty() {
        issues.push("✅ Deploy verificado correctamente".to_string());
    }

    Ok(issues)
}

/// Refresca la dimensión RUC desde ventas
pub async fn refresh_dim_ruc(pool: &SqlitePool) -> Result<usize> {
    sqlx::query(r#"
        INSERT INTO dim_ruc (ruc, nom_cliente, departamento, provincia, distrito, n_clientes_unicos, actualizado_en)
        SELECT 
            doc_cliente as ruc,
            MAX(nom_cliente) as nom_cliente,
            MAX(departamento),
            MAX(provincia),
            MAX(distrito),
            COUNT(DISTINCT id_cliente) as n_clientes_unicos,
            datetime('now')
        FROM ventas 
        WHERE doc_cliente GLOB '[0-9]*' AND doc_cliente != ''
        GROUP BY doc_cliente
        ON CONFLICT(ruc) DO UPDATE SET
            nom_cliente = excluded.nom_cliente,
            departamento = excluded.departamento,
            provincia = excluded.provincia,
            distrito = excluded.distrito,
            n_clientes_unicos = excluded.n_clientes_unicos,
            actualizado_en = datetime('now')
    "#)
    .execute(pool)
    .await?;
    
    Ok(1)
}

/// Verifica integridad post-captura (más estricto que verify_integrity)
pub async fn verify_post_capture(pool: &SqlitePool) -> Result<Vec<String>> {
    let mut issues = Vec::new();

    // Checksums mensuales
    let checksum_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mes_checksums").fetch_one(pool).await?;
    let month_count: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT mes_ref) FROM ventas").fetch_one(pool).await?;
    if checksum_count < (month_count as f64 * 0.9) as i64 {
        issues.push(format!("⚠️ Checksums incompletos: {} de {} meses", checksum_count, month_count));
    }

    // Sin duplicados literales
    let literal_dups: i64 = sqlx::query_scalar(r#"
        SELECT COUNT(*) FROM (
            SELECT id_articulo, folio_unico, cantidad, soles,
                   ROW_NUMBER() OVER(PARTITION BY id_articulo, folio_unico, cantidad, soles ORDER BY id) as rn
            FROM ventas WHERE folio_unico != ''
        ) WHERE rn > 1
    "#).fetch_one(pool).await?;
    if literal_dups > 0 {
        issues.push(format!("⚠️ Duplicados literales encontrados: {}", literal_dups));
    }

    // Consistencia fecha/mes
    let date_mismatch: i64 = sqlx::query_scalar(r#"
        SELECT COUNT(*) FROM ventas 
        WHERE substr(fecha_orig, 1, 7) != mes_ref
          AND fecha_orig != ''
          AND mes_ref != ''
    "#).fetch_one(pool).await?;
    if date_mismatch > 0 {
        issues.push(format!("⚠️ Fechas inconsistentes con mes: {}", date_mismatch));
    }

    if issues.is_empty() {
        issues.push("✅ Integridad post-captura verificada".to_string());
    }

    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn test_verify_deploy_empty_db() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        
        // Crear tablas básicas
        sqlx::query(crate::db::schema::CREATE_TABLE_SQL)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(crate::db::schema::CREATE_AUDIT_TABLES_SQL)
            .execute(&pool)
            .await
            .unwrap();
        for idx in crate::db::schema::CREATE_INDEXES_SQL {
            sqlx::query(idx).execute(&pool).await.unwrap();
        }
        
        let issues = verify_deploy(&pool).await.unwrap();
        assert!(issues.iter().any(|s| s.contains("BD vacía")));
    }
}
