// Las columnas de fecha que se guardan como texto deben quedar en ISO.
//
// Regression: `fec_cargo` se guardaba crudo (`g(c_fec)`), sin pasar por el
// parser, y quedaron 74.380 filas en dd/mm/yyyy mientras `fecha_orig` ya
// estaba en ISO. Como el SQL compara fechas como texto, un rango sobre esa
// columna devuelve CERO filas sin dar error:
//
//     BETWEEN '2015-01-01' AND '2015-12-31'  ->  0 filas
//     BETWEEN '01/01/2015' AND '31/12/2015'  ->  74.380 filas
//
// Y como `fec_cargo` viene 97% vacio del ERP (solo lo completa en parte de
// las notas de credito), el caso NO es "llenar lo que falta": es que lo que
// llega tiene que quedar en el mismo formato que el resto.
#[test]
fn test_fechas_de_texto_quedan_en_iso() {
    let tmp = std::env::temp_dir().join("test_fechas_iso_g360");
    std::fs::create_dir_all(&tmp).unwrap();
    let csv = tmp.join("ventas_2026_01.csv");
    // 5 filas que ejercitan: dd/mm, dd-mm, ISO, vacio e ilegible.
    std::fs::write(
        &csv,
        concat!(
            "ANHO,MES,ID_CLIENTE,DOC_CLIENTE,NOM_CLIENTE,ID_LINEA,NOM_LINEA,ID_ARTICULO,NOM_ARTICULO,",
            "TPO_DOC,SERIE_DOC,NRO_DOC,REFERENCIA,FECHA_ORIG,FECHA_REF,FECHA_VENC,FEC_CARGO,",
            "MONEDA,CANTIDAD,CANTIDAD FAE,SOLES,DOLARES\n",
            "2026,01,00012345,12345678901,CLIENTE UNO,0101,LINEA A,SKU1,ARTICULO UNO,F01,201,100,,15/01/2024,15/01/2024,15/01/2024,15/01/2024,Soles,10,10,100.00,0.00\n",
            "2026,01,00012345,12345678901,CLIENTE UNO,0101,LINEA A,SKU2,ARTICULO DOS,F01,201,101,,16/01/2024,16-01-2024,2024-01-16,16-01-2024,Soles,10,10,100.00,0.00\n",
            "2026,01,00012345,12345678901,CLIENTE DOS,0102,LINEA B,SKU3,ARTICULO TRES,F01,201,102,,2024-01-15,2024-01-15,2024-01-15,2024-01-15,Soles,10,10,100.00,0.00\n",
            // fec_cargo vacio: se conserva vacio. NO se rellena (el ERP no lo mando).
            "2026,01,00012345,12345678901,CLIENTE DOS,0102,LINEA B,SKU4,ARTICULO CUATRO,F01,201,103,,17/01/2024,17/01/2024,17/01/2024,,Soles,10,10,100.00,0.00\n",
            // ilegible: se conserva tal cual. Preferimos un dato raro a perder la fila.
            "2026,01,00012345,12345678901,CLIENTE DOS,0102,LINEA B,SKU5,ARTICULO CINCO,F01,201,104,,18/01/2024,18/01/2024,18/01/2024,basura,Soles,10,10,100.00,0.00\n",
        ),
    )
    .unwrap();

    let (ventas, _stats) = g360_db_ventas::processor::parser::parse_export_csv(&csv)
        .map_err(|e| eprintln!("parse: {e}"))
        .expect("parse");
    assert_eq!(ventas.len(), 5, "deben parsearse las 5 filas");

    let es_iso = |s: &str| {
        s.len() == 10
            && s.as_bytes()[4] == b'-'
            && s.as_bytes()[7] == b'-'
            && s.as_bytes()
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    };

    // fecha_orig siempre fue ISO (el tipo NaiveDate lo garantiza).
    for v in &ventas {
        assert!(
            es_iso(&v.fecha_orig.to_string()),
            "fecha_orig no ISO: {}",
            v.fecha_orig
        );
    }

    for (i, v) in ventas.iter().enumerate().take(3) {
        assert!(
            es_iso(&v.fec_cargo),
            "fila {i}: fec_cargo quedo {:?}, deberia ser ISO",
            v.fec_cargo
        );
        assert!(
            v.fecha_ref.as_deref().map(es_iso).unwrap_or(false),
            "fila {i}: fecha_ref quedo {:?}",
            v.fecha_ref
        );
        assert!(
            v.fecha_venc.as_deref().map(es_iso).unwrap_or(false),
            "fila {i}: fecha_venc quedo {:?}",
            v.fecha_venc
        );
    }

    // Convierte bien el contenido, no solo la forma.
    assert_eq!(ventas[0].fec_cargo, "2024-01-15", "dd/mm/yyyy -> ISO");
    assert_eq!(ventas[1].fec_cargo, "2024-01-16", "dd-mm-yyyy -> ISO");
    assert_eq!(ventas[2].fec_cargo, "2024-01-15", "ISO se mantiene");

    // Vacio queda vacio: el ERP no lo mando y no se inventa.
    assert_eq!(
        ventas[3].fec_cargo, "",
        "fec_cargo vacio no debe rellenarse"
    );

    // Ilegible se conserva tal cual.
    assert_eq!(
        ventas[4].fec_cargo, "basura",
        "un valor ilegible debe conservarse, no convertirse en vacio"
    );
}
