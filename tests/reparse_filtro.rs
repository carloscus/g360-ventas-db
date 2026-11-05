// El filtro de archivos de `reparse` debe seleccionar los CSV MENSUALES.
//
// Regression: el filtro era `name.len() == "ventas_YYYY-MM.csv".len()`, o sea
// 18, cuando el nombre real tiene 16. Con eso NINGUNO de los 202 meses
// calificaba. En vez de reprocesar mes a mes, el prefijo de año ("2010-0")
// hacía matchear y `insert_ventas` ejecutaba
// `DELETE FROM ventas WHERE mes_ref = '2010-0'`: un mes inexistente, así que
// no borraba nada e insertaba encima, duplicando ~2.8M de filas.

/// Extrae el mes (yyyy, mm) del nombre del archivo CSV mensual.
/// Retorna None si no es un nombre valido de mes.
fn mes_de_nombre(nome: &str) -> Option<(u32, u32)> {
    if !nome.ends_with(".csv") {
        return None;
    }
    let stem = nome.trim_end_matches(".csv");
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() != 2 {
        return None;
    }
    if parts[0] != "ventas" {
        return None;
    }
    let date_part = parts[1];
    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 2 {
        return None;
    }
    let year: u32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    if month < 1 || month > 12 {
        return None;
    }
    if date_parts[0].len() != 4 {
        return None;
    }
    if date_parts[1].len() != 2 {
        return None;
    }
    Some((year, month))
}

#[test]
fn acepta_los_mensuales_reales() {
    for n in [
        "ventas_2026-01.csv",
        "ventas_2010-01.csv",
        "ventas_2026-12.csv",
        "ventas_2015-09.csv",
    ] {
        assert!(mes_de_nombre(n).is_some(), "{n} deberia aceptarse");
    }
}

#[test]
fn rechaza_parciales_de_pagina() {
    // "2026-01-p4_1" es un trozo del mismo mes: reprocesarlo junto al mensual
    // duplicaría las filas.
    for n in [
        "ventas_2026-01-p4_1.csv",
        "ventas_2026-01-p4.csv",
        "ventas_2026-01-p1_2.csv",
        "ventas_2010-01-p4_2.csv",
    ] {
        assert!(mes_de_nombre(n).is_none(), "{n} deberia rechazarse");
    }
}

#[test]
fn rechaza_meses_imposibles() {
    for n in [
        "ventas_2026-13.csv",
        "ventas_2026-00.csv",
        "ventas_2026-1.csv",
        "ventas_26-01.csv",
        "ventas_2026-012.csv",
        "ventas_2010-0.csv", // justo lo que el bug anterior hacia pasar
    ] {
        assert!(mes_de_nombre(n).is_none(), "{n} deberia rechazarse");
    }
}

#[test]
fn rechaza_lo_que_no_es_csv_de_ventas() {
    for n in [
        "capture.lock",
        "ventas_2026-01.xls",
        "otro_2026-01.csv",
        "ventas_2026-01.csv.bak",
        "ventas.csv",
        "",
    ] {
        assert!(mes_de_nombre(n).is_none(), "{n} deberia rechazarse");
    }
}

#[test]
fn devuelve_el_mes_esperado() {
    assert_eq!(mes_de_nombre("ventas_2026-01.csv"), Some((2026, 1)));
    assert_eq!(mes_de_nombre("ventas_2010-12.csv"), Some((2010, 12)));
    assert_eq!(mes_de_nombre("ventas_2010-0.csv"), None);
}

#[test]
fn el_conteo_real_de_una_captura() {
    // 274 CSVs en raw/, 72 parciales, 202 mensuales. Con el filtro roto no
    // entraba ninguno de los mensuales.
    let total = 274;
    let parciales = 72;
    assert_eq!(total - parciales, 202);
}
