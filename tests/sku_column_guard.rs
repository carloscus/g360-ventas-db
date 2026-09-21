// Guard de integridad: detectar XLS con la columna SKU (ID_ARTICULO) guardada
// como NUMERO. En ese caso Excel pudo borrar ceros a la izquierda (011019 ->
// 11019) y la importacion manual crearia SKUs falsos que colisionan con otros
// de distinta linea.
//
// Fixtures generados por tests/make_sku_fixtures.py:
//   - sku_text.xlsx    : SKU como TEXTO  -> ERP correcto
//   - sku_numeric.xlsx : SKU como NUMERO -> re-guardado en Excel
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn sku_texto_no_dispara_alerta() {
    let path = fixture("sku_text.xlsx");
    let insp = g360_db_ventas::processor::xls::inspect_sku_column(&path)
        .expect("inspeccionar sku_text.xlsx");
    assert!(insp.header_found, "debe encontrar la cabecera ID_ARTICULO");
    assert!(
        !insp.has_numeric_sku(),
        "SKU como texto NO debe disparar la alerta (numeric_cells={})",
        insp.numeric_cells
    );
    assert!(insp.string_cells >= 3, "debe contar las celdas de SKU como texto");
}

#[test]
fn sku_numerico_dispara_alerta() {
    let path = fixture("sku_numeric.xlsx");
    let insp = g360_db_ventas::processor::xls::inspect_sku_column(&path)
        .expect("inspeccionar sku_numeric.xlsx");
    assert!(insp.header_found, "debe encontrar la cabecera ID_ARTICULO");
    assert!(
        insp.has_numeric_sku(),
        "SKU como numero DEBE disparar la alerta"
    );
    assert_eq!(insp.numeric_cells, 3, "las 3 filas tienen SKU numerico");
    // El mensaje debe ser accionable y mencionar el riesgo de ceros.
    let msg = insp.warning_message();
    assert!(msg.contains("NÚMERO") || msg.contains("NUMERO"));
    assert!(msg.contains("ceros"));
}

#[test]
fn xls_to_csv_deja_skus_texto_intactos() {
    // La conversion no debe tocar los ceros a la izquierda del ERP.
    let xlsx = fixture("sku_text.xlsx");
    let out = std::env::temp_dir().join("g360_test_sku_text.csv");
    let n = g360_db_ventas::processor::xls::xls_to_csv(&xlsx, &out).expect("convertir");
    assert!(n >= 3);
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains("011019"), "debe preservar 011019 literal:\n{content}");
    assert!(content.contains("011028"), "debe preservar 011028 literal:\n{content}");
    let _ = std::fs::remove_file(&out);
}
