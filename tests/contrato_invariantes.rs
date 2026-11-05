// Tests de invariantes del contrato: normalización de id_pedido, mes_ref, fechas.
// Fixture dorado: input CSV -> salida esperada.
use g360_db_ventas::processor::parser::parse_export_csv;
use std::io::Write;
use tempfile::TempDir;

/// Helper: escribe un CSV minimal con las columnas requeridas y lo parsea.
fn write_csv(tmp: &TempDir, header: &str, rows: &str) -> std::path::PathBuf {
    let content = format!("{}\n{}", header, rows);
    let path = tmp.path().join("ventas_test.csv");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(content.as_bytes())
        .unwrap();
    path
}

#[test]
fn test_id_pedido_sin_espacios() {
    let tmp = TempDir::new().unwrap();
    let csv = write_csv(
        &tmp,
        "ID_ARTICULO,ID_CLIENTE,FECHA_ORIG,TPO_DOC,SERIE_DOC,NRO_DOC,ID_PEDIDO",
        "SKU001,CLI001,15/01/2024,F01,204,12345,SADIE 25",
    );
    let (rows, _stats) = parse_export_csv(&csv).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id_pedido, "SADIE_25");
}

#[test]
fn test_id_pedido_multiple_espacios() {
    let tmp = TempDir::new().unwrap();
    let csv = write_csv(
        &tmp,
        "ID_ARTICULO,ID_CLIENTE,FECHA_ORIG,TPO_DOC,SERIE_DOC,NRO_DOC,ID_PEDIDO",
        "SKU001,CLI001,15/01/2024,F01,204,12345,ORD 123 ABC",
    );
    let (rows, _stats) = parse_export_csv(&csv).unwrap();
    assert_eq!(rows[0].id_pedido, "ORD_123_ABC");
}

#[test]
fn test_mes_ref_derivado_de_fecha_orig() {
    let tmp = TempDir::new().unwrap();
    // El archivo se llama "ventas_2024-01" pero la fecha es de febrero.
    // mes_ref debe derivarse de fecha_orig, no del nombre del archivo.
    let csv = write_csv(
        &tmp,
        "ID_ARTICULO,ID_CLIENTE,FECHA_ORIG,TPO_DOC,SERIE_DOC,NRO_DOC",
        "SKU001,CLI001,28/02/2024,F01,204,99999",
    );
    let (rows, _stats) = parse_export_csv(&csv).unwrap();
    assert_eq!(rows[0].mes_ref, "2024-02");
    assert_eq!(rows[0].anho, 2024);
    assert_eq!(rows[0].mes, 2);
}

#[test]
fn test_fechas_iso_convertidas() {
    let tmp = TempDir::new().unwrap();
    // Input con formato ISO (yyyy-mm-dd) tambien debe parsearse.
    let csv = write_csv(
        &tmp,
        "ID_ARTICULO,ID_CLIENTE,FECHA_ORIG,TPO_DOC,SERIE_DOC,NRO_DOC",
        "SKU001,CLI001,2024-03-15,F01,204,55555",
    );
    let (rows, _stats) = parse_export_csv(&csv).unwrap();
    assert_eq!(rows[0].fecha_orig.to_string(), "2024-03-15");
    assert_eq!(rows[0].mes_ref, "2024-03");
}

#[test]
fn test_calidad_stats_cuenta_invalidos() {
    let tmp = TempDir::new().unwrap();
    // Una fila con fecha invalida debe contar en stats
    let csv = write_csv(
        &tmp,
        "ID_ARTICULO,ID_CLIENTE,FECHA_ORIG,TPO_DOC,SERIE_DOC,NRO_DOC",
        "SKU001,CLI001,32/01/2024,F01,204,11111\nSKU002,CLI002,15/01/2024,F01,204,22222",
    );
    let (_rows, stats) = parse_export_csv(&csv).unwrap();
    assert_eq!(stats.invalid_dates, 1);
    assert_eq!(stats.total_rows, 2);
}
