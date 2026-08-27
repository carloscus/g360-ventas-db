// Test xls_to_csv con calamine contra XLS reales descargados
use std::path::PathBuf;

#[test]
fn test_calamine_vs_xlrd_enero() {
    let xls = PathBuf::from(r"C:\Users\ccusi\Downloads\dgvVentas (4).xls");
    if !xls.exists() { eprintln!("skip: no existe {}", xls.display()); return; }
    let csv = std::env::temp_dir().join("test_enero_calamine.csv");
    let rows = g360_db_ventas::processor::xls::xls_to_csv(&xls, &csv).expect("xls_to_csv fallo");
    println!("filas escritas: {rows} (header+data, xlrd vio 15495 totales)");
    assert!(rows >= 15490, "filas inesperadas {rows}");

    // Sumar SOLES/CANTIDAD con parser CSV real (quoting correcto por comas en nombres)
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(&csv)
        .expect("abrir csv");
    let headers = reader.headers().expect("headers").clone();
    let idx_cant = headers.iter().position(|h| h.trim() == "CANTIDAD").expect("col CANTIDAD");
    let idx_soles = headers.iter().position(|h| h.trim() == "SOLES").expect("col SOLES");
    let mut soles_sum = 0f64;
    let mut dol_sum = 0f64;
    let mut cant_sum = 0f64;
    let mut cnt = 0usize;
    let headers_lc: Vec<String> = headers.iter().map(|h| h.trim().to_uppercase()).collect();
    let idx_dol = headers_lc.iter().position(|h| h == "DOLARES").expect("col DOLARES");
    for rec in reader.records() {
        let r = rec.expect("record");
        soles_sum += r.get(idx_soles).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        dol_sum += r.get(idx_dol).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        cant_sum += r.get(idx_cant).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        cnt += 1;
    }
    println!("data filas={cnt} soles={soles_sum:.2} dolares={dol_sum:.2} cantidad={cant_sum:.2}");
    // Valores de referencia confirmados por el usuario contra resumen intranet:
    // cantidad=2814659.579 soles=12419650.04 dolares=3422659.84
    assert!(cnt >= 15490, "registros inesperados {cnt}");
    assert!((soles_sum - 12419650.04).abs() < 200.0, "soles difiere: {soles_sum}");
    assert!((cant_sum - 2814659.58).abs() < 200.0, "cantidad difiere: {cant_sum}");
    assert!((dol_sum - 3422659.84).abs() < 500.0, "dolares difiere: {dol_sum}");
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
