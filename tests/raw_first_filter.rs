// Tests raw-first: el parser conserva TODO el export del ERP y el filtro de
// lineas (allowlist + regla NC/ND) se aplica solo al subir a Supabase via
// config::with_line_filter. Verifica:
//   1. parse_export_csv NO filtra por linea (raw completo en BD local)
//   2. el WHERE SQL del upload coincide con el espejo Rust de la regla
//   3. stats_cache queda alineada con lo que sube a Supabase, no con el raw
use g360_db_ventas::config::{is_allowed_line, supabase_line_filter_sql, with_line_filter};
use g360_db_ventas::db::schema::{
    CREATE_AUDIT_TABLES_SQL, CREATE_INDEXES_SQL, CREATE_STATS_CACHE_SQL, CREATE_TABLE_SQL,
};
use g360_db_ventas::processor::parser::parse_export_csv;
use std::collections::HashMap;

const CSV: &str = "\
ANHO,MES,ID_CLIENTE,DOC_CLIENTE,NOM_CLIENTE,ID_LINEA,NOM_LINEA,ID_ARTICULO,NOM_ARTICULO,TPO_DOC,SERIE_DOC,NRO_DOC,REFERENCIA,FECHA_ORIG,MONEDA,CANTIDAD,CANTIDAD FAE,SOLES,DOLARES
2026,01,00012345,12345678901,CLIENTE UNO,0101,LINEA A,SKU1,ARTICULO UNO,F01,201,100,,01/09/2026,Soles,10,10,100.00,0.00
2026,01,00012345,12345678901,CLIENTE UNO,0103,LINEA EXCLUIDA,SKU2,ARTICULO DOS,F01,201,101,,01/09/2026,Soles,5,5,50.00,0.00
2026,01,00012345,12345678901,CLIENTE UNO,0103,LINEA EXCLUIDA,SKU2,ARTICULO DOS,NCR,F02,900,F01/201-100,01/09/2026,Soles,-2,-2,-20.00,0.00
2026,01,00012345,12345678901,CLIENTE UNO,0101,LINEA A,SKU1,ARTICULO UNO,NCR,F02,901,F01/201-100,01/09/2026,Soles,-1,-1,-10.00,0.00
2026,01,00012345,12345678901,CLIENTE UNO,0103,LINEA EXCLUIDA,SKU2,ARTICULO DOS,NCR,F02,902,F01/201-101,01/09/2026,Soles,-1,-1,-10.00,0.00
2026,01,00012345,12345678901,CLIENTE UNO,0101,LINEA A,SKU1,ARTICULO UNO,NCR,F02,903,,02/09/2026,Soles,-1,-1,-10.00,0.00
";

fn es_nc(tpo: &str) -> bool {
    let t = tpo.to_uppercase();
    t.contains("NCR") || t.contains("NDB")
}

/// Espejo Rust de la regla SQL (config::supabase_line_filter_sql):
/// una fila sube a Supabase si su linea es permitida, O si es NC/ND cuya
/// FACTURA DE VENTA (F01) existe en la BD local con linea permitida (lookup
/// global: cualquier fila no-NC/ND con (serie,nro) y linea permitida habilita).
fn esperadas_upload(vs: &[g360_db_ventas::Venta]) -> usize {
    let mut ref_ok: HashMap<(String, String), bool> = HashMap::new();
    for v in vs {
        if !es_nc(&v.tpo_doc) && is_allowed_line(&v.id_linea)
            && !v.serie_doc.is_empty() && !v.nro_doc.is_empty()
        {
            ref_ok.insert((v.serie_doc.clone(), v.nro_doc.clone()), true);
        }
    }
    vs.iter()
        .filter(|v| {
            is_allowed_line(&v.id_linea)
                || (es_nc(&v.tpo_doc)
                    && !v.factura_ref_serie.is_empty()
                    && !v.factura_ref_nro.is_empty()
                    && ref_ok
                        .get(&(v.factura_ref_serie.clone(), v.factura_ref_nro.clone()))
                        .copied()
                        .unwrap_or(false))
        })
        .count()
}

#[test]
fn parser_raw_first_conserva_todas_las_filas() {
    let dir = std::env::temp_dir().join("g360_raw_first_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("ventas_2026_01.csv");
    std::fs::write(&csv, CSV).unwrap();

    let (ventas, _stats) = parse_export_csv(&csv).expect("parse");
    // Las 6 filas deben llegar a la BD local, incluidas las de linea 0103
    // (fuera del allowlist por defecto) y la NC sin referencia.
    assert_eq!(ventas.len(), 6, "raw-first: el parser NO debe filtrar por linea");
    assert!(
        ventas.iter().any(|v| v.id_linea == "03" && v.tpo_doc == "F01"),
        "la fila de linea no permitida debe conservarse en el raw (id_linea normalizada sin prefijo)"
    );
    assert!(
        ventas.iter().any(|v| v.tpo_doc == "NCR" && v.factura_ref_serie.is_empty()),
        "la NC sin factura referenciada debe conservarse en el raw"
    );
    // La regla NC/ND se sigue derivando igual (referencia -> serie/nro)
    let nc900 = ventas.iter().find(|v| v.nro_doc == "900").unwrap();
    assert_eq!(nc900.factura_ref_serie, "201");
    assert_eq!(nc900.factura_ref_nro, "100");
}

#[tokio::test]
async fn filtro_upload_sql_coincide_con_espejo_rust() {
    let dir = std::env::temp_dir().join("g360_raw_first_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("ventas_2026_01.csv");
    std::fs::write(&csv, CSV).unwrap();
    let (ventas, _stats) = parse_export_csv(&csv).expect("parse");

    // BD en memoria con el schema real del proyecto
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool memoria");
    sqlx::query(CREATE_TABLE_SQL).execute(&pool).await.unwrap();
    for idx in CREATE_INDEXES_SQL {
        sqlx::query(idx).execute(&pool).await.unwrap();
    }
    sqlx::query(CREATE_STATS_CACHE_SQL).execute(&pool).await.unwrap();
    sqlx::query(CREATE_AUDIT_TABLES_SQL).execute(&pool).await.unwrap();

    g360_db_ventas::db::writer::insert_ventas(&pool, &ventas, false)
        .await
        .expect("insert_ventas");

    // Raw completo en local
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ventas")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total as usize, ventas.len(), "la BD local debe tener TODO el raw");

    // El WHERE del upload (SQL) debe coincidir con el espejo Rust de la regla
    let upload_where = with_line_filter("");
    let elegibles: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM ventas {}", upload_where))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        elegibles as usize,
        esperadas_upload(&ventas),
        "el filtro SQL del upload debe coincidir con la regla Rust"
    );
    assert!(elegibles <= total, "el filtro nunca puede ampliar el raw");

    // Con allowlist por defecto: 0101 permitida, 0103 no -> suben 4 de 6:
    //   fila 1 (F01 linea 0101), fila 3 (NC ref factura permitida),
    //   fila 4 (NC linea 0101), fila 6 (NC sin ref pero linea 0101).
    // Excluidas: fila 2 (F01 linea 0103) y fila 5 (NC ref factura 0103).
    if !is_allowed_line("0103") {
        assert_eq!(elegibles, 4, "escenario esperado con allowlist por defecto");

        // stats_cache alineada con lo vital (no con el raw)
        g360_db_ventas::db::writer::refresh_stats_cache(&pool).await.unwrap();
        let st: f64 = sqlx::query_scalar("SELECT value FROM stats_cache WHERE key = 'total_records'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(st as i64, 4, "stats_cache debe reflejar lo que sube a Supabase");

        // La combinacion con sync/retencion mantiene el filtro obligatorio
        let w = with_line_filter("WHERE mes_ref >= '2026-01'");
        assert!(w.starts_with("WHERE mes_ref >= '2026-01' AND ("));
        let combinada: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM ventas {}", w))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(combinada, elegibles, "todas las filas son de 2026_01");
    }
}

#[test]
fn with_line_filter_combina_y_falla_cerrado() {
    let f = supabase_line_filter_sql();
    assert!(f.contains("id_linea IN ("), "filtro por linea permitida (exact-match 2 chars)");
    assert!(f.contains("EXISTS (SELECT 1 FROM ventas f"), "regla NC/ND");

    assert!(with_line_filter("").starts_with("WHERE ("));
    let w2 = with_line_filter("WHERE mes_ref >= '2026-01'");
    assert!(w2.starts_with("WHERE mes_ref >= '2026-01' AND ("));
    let w3 = with_line_filter("mes_ref >= '2026-01'");
    assert!(w3.starts_with("WHERE mes_ref >= '2026-01' AND ("));
}
