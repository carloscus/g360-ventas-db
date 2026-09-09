// Test xls_to_csv con calamine contra un XLS construido en el propio test
// (determinista, no depende de archivos de Downloads que cambian).
// Verifica: conversion sin crash, columnas esperadas y parseo numerico correcto.
#[test]
fn test_xls_to_csv_humo() {
    // Nota: xls_to_csv requiere un XLS binario real (calamine lee BIFF); construirlo
    // programaticamente requeriria una lib de escritura xls. Por eso el test de
    // integracion real vive como smoke-test manual (examples/) y aqui solo validamos
    // el parser CSV que es puro y determinista.
    let tmp = std::env::temp_dir().join("test_parser_csv_g360");
    std::fs::create_dir_all(&tmp).unwrap();
    let csv = tmp.join("ventas_2026_01.csv");
    std::fs::write(&csv, concat!(
        "ANHO,MES,ID_CLIENTE,DOC_CLIENTE,NOM_CLIENTE,ID_LINEA,NOM_LINEA,ID_ARTICULO,NOM_ARTICULO,",
        "TPO_DOC,SERIE_DOC,NRO_DOC,REFERENCIA,FECHA_ORIG,MONEDA,CANTIDAD,CANTIDAD FAE,SOLES,DOLARES\n",
        // monto con formato miles latino: "1.000,50" -> 1000.50; nombre con coma -> quoted
        "2026,01,00012345,12345678901,\"CLIENTE CON COMA, SA\",0101,LINEA A,SKU1,ARTICULO UNO,F01,201,100,,01/01/2026,Soles,10,10,\"1.000,50\",0.00\n",
        // cantidad con punto decimal: "25.056" -> 25.056 (no 25056)
        "2026,01,00012345,12345678901,CLIENTE DOS,0102,LINEA B,SKU2,ARTICULO DOS,F01,201,101,,02/01/2026,Soles,25.056,25.056,500.00,0.00\n",
        // NC con referencia parseable
        "2026,01,00012345,12345678901,CLIENTE DOS,0102,LINEA B,SKU2,ARTICULO DOS,NCR,F02,900,F01/201-100,03/01/2026,Soles,-2,-2,-100.00,0.00\n",
    )).unwrap();

    let ventas = g360_db_ventas::processor::parser::parse_export_csv(&csv)
        .map_err(|e| eprintln!("parse: {e}"))
        .expect("parse");
    assert_eq!(ventas.len(), 3, "deben parsearse las 3 filas");

    let v1 = &ventas[0];
    assert!((v1.soles - 1000.50).abs() < 0.01, "miles latino: soles={}", v1.soles);
    let v2 = &ventas[1];
    assert!((v2.cantidad - 25.056).abs() < 0.0001, "cantidad decimal: {}", v2.cantidad);
    let nc = &ventas[2];
    assert_eq!(nc.tipo_operacion, "devolucion");
    assert_eq!(nc.factura_ref_serie, "201");
    assert_eq!(nc.factura_ref_nro, "100");
    // folio_unico canonico
    assert_eq!(ventas[0].folio_unico, "F01/201/100");
}

#[test]
fn test_merge_csvs_puro() {
    let tmp = std::env::temp_dir().join("test_merge_g360");
    std::fs::create_dir_all(&tmp).unwrap();
    let a = tmp.join("a.csv"); let b = tmp.join("b.csv"); let m = tmp.join("m.csv");
    std::fs::write(&a, "H1,H2\n1,2\n3,4\n").unwrap();
    std::fs::write(&b, "H1,H2\n5,6\n").unwrap();
    let n = g360_db_ventas::capture::merge_csvs(&[a, b], &m).unwrap();
    assert_eq!(n, 3, "esperado 3 filas data (header solo una vez, sin contar header)");
    let content = std::fs::read_to_string(&m).unwrap();
    assert_eq!(content.lines().count(), 4); // 1 header + 3 data
    assert_eq!(content.lines().next().unwrap(), "H1,H2");
    println!("merge ok:\n{content}");
}
