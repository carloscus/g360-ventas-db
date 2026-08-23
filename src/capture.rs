use anyhow::{anyhow, Result};
use chrono::{Datelike, Duration, NaiveDate};
use headless_chrome::{Browser, LaunchOptionsBuilder};
use std::time::Duration as StdDuration;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::browser::captor::{capture_month, MonthRange};
use crate::config::{raw_dir, SLEEP_BETWEEN_MONTHS};
use crate::db::writer::{dedup_ventas, init_pool, insert_ventas};
use crate::processor::parser::parse_export_csv;

pub async fn run_batch_history(_sd: &str, _ed: &str, daily: bool) -> Result<()> {
    let raw = raw_dir();
    std::fs::create_dir_all(&raw)?;
    let mut pending = Vec::new();
    let now = chrono::Local::now().naive_local();
    let today = now.date();

    if daily {
        let pool = init_pool().await?;
        let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
            .fetch_one(&pool)
            .await
            .unwrap_or(None);
        let last_date = last
            .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
            .unwrap_or(NaiveDate::from_ymd_opt(2021, 1, 1).unwrap());
        // Re-captura 1 dia extra hacia atras (colchon) para cubrir ventas que
        // hayan entrado despues de la ultima captura del dia anterior.
        let floor = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let mut d = if last_date > floor { last_date - Duration::days(1) } else { last_date };
        while d <= today {
            let label = d.format("%Y-%m-%d").to_string();
            pending.push(MonthRange {
                start: d,
                end: d,
                label,
            });
            d = d + Duration::days(1);
        }
        info!(
            "daily sync: last={} hoy={} -> {} dias (con 1 de margen)",
            last_date,
            today,
            pending.len()
        );
    } else {
        let start_default = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let s: NaiveDate = if _sd.is_empty() || _sd.contains("--") {
            start_default
        } else {
            let sd_txt = _sd.to_string();
            NaiveDate::parse_from_str(&sd_txt, "%Y-%m-%d").ok().unwrap_or(start_default)
        };
        let e: NaiveDate = if _ed.is_empty() || _ed.contains("--") {
            today
        } else {
            let ed_txt = _ed.to_string();
            NaiveDate::parse_from_str(&ed_txt, "%Y-%m-%d").ok().unwrap_or(today)
        };
        if s > e {
            info!("rango invalido (desde > hasta): {} -> {}", _sd, _ed);
            return Ok(());
        }
        let ranges = gen_month_range(s.year(), s.month(), e.year(), e.month());
        for r in ranges {
            let csv = raw.join(format!("ventas_{}.csv", r.label));
            let has = csv.exists()
                && std::fs::metadata(&csv)
                    .map(|m| m.len() > 1000)
                    .unwrap_or(false);
            if has {
                continue;
            }
            pending.push(r);
        }
        info!(
            "batch_history ({} -> {}): {} pendientes",
            _sd,
            _ed,
            pending.len()
        );
    }

    for (i, r) in pending.iter().enumerate() {
        info!("  [{}] {}", i + 1, r.label);
    }
    if pending.is_empty() {
        info!("Nada pendiente");
        return Ok(());
    }

    let mut ok = Vec::new();
    let mut fail = Vec::new();

    for (idx, m) in pending.iter().enumerate() {
        info!("=== {}/{}: {} ===", idx + 1, pending.len(), m.label);
        let mut success = false;
        for attempt in 1..=3 {
            info!("  intento {}/3", attempt);
            let browser = match Browser::new(
                LaunchOptionsBuilder::default()
                    .headless(true)
                    .build()
                    .map_err(|e| anyhow!("Launch: {}", e))?,
            )
            .map_err(|e| anyhow!("{:?}", e))
            {
                Ok(b) => b,
                Err(e) => {
                    warn!("  browser fail: {}", e);
                    sleep(StdDuration::from_secs(5)).await;
                    continue;
                }
            };
            match capture_month(&browser, m, &raw).await {
                Ok(xls) => {
                    let csv = raw.join(format!("ventas_{}.csv", m.label));
                    let py = format!("import xlrd, csv; wb=xlrd.open_workbook(r'{}'); sh=wb.sheet_by_index(0); w=csv.writer(open(r'{}','w',newline='',encoding='utf-8')); [w.writerow([sh.cell_value(r,c) for c in range(sh.ncols)]) for r in range(sh.nrows)]", xls.display(), csv.display());
                    let _ = std::process::Command::new(
                        if std::process::Command::new("python3")
                            .arg("--version")
                            .output()
                            .is_ok()
                        {
                            "python3"
                        } else {
                            "python"
                        },
                    )
                    .arg("-c")
                    .arg(&py)
                    .output();
                    success = true;
                    break;
                }
                Err(e) => {
                    warn!("  fail {}: {}", attempt, e);
                    sleep(StdDuration::from_secs(3)).await;
                }
            }
        }

        if !success {
            let mut recovered = false;
            for parts in [2usize, 4, 10] {
                warn!("  {} fallo, probando {} partes", m.label, parts);
                let chunks = split_month_n(m, parts);
                let mut ok_chunks = 0;
                for h in &chunks {
                    let mut h_ok = false;
                    for ha in 1..=2 {
                        let browser_h = match Browser::new(
                            LaunchOptionsBuilder::default()
                                .headless(true)
                                .build()
                                .map_err(|e| anyhow!("Launch: {}", e))?,
                        )
                        .map_err(|e| anyhow!("{:?}", e))
                        {
                            Ok(b) => b,
                            Err(e) => {
                                warn!("  chunk {} browser fail: {}", h.label, e);
                                sleep(StdDuration::from_secs(3)).await;
                                continue;
                            }
                        };
                        match capture_month(&browser_h, h, &raw).await {
                            Ok(xls_h) => {
                                let csv_h = raw.join(format!("ventas_{}.csv", h.label));
                                let py_h = format!("import xlrd, csv; wb=xlrd.open_workbook(r'{}'); sh=wb.sheet_by_index(0); w=csv.writer(open(r'{}','w',newline='',encoding='utf-8')); [w.writerow([sh.cell_value(r,c) for c in range(sh.ncols)]) for r in range(sh.nrows)]", xls_h.display(), csv_h.display());
                                let _ = std::process::Command::new(
                                    if std::process::Command::new("python3")
                                        .arg("--version")
                                        .output()
                                        .is_ok()
                                    {
                                        "python3"
                                    } else {
                                        "python"
                                    },
                                )
                                .arg("-c")
                                .arg(&py_h)
                                .output();
                                h_ok = true;
                                break;
                            }
                            Err(e) => {
                                warn!("  chunk {} intento {} fail: {}", h.label, ha, e);
                                sleep(StdDuration::from_secs(2)).await;
                            }
                        }
                    }
                    if h_ok {
                        ok_chunks += 1;
                    } else {
                        break;
                    }
                    sleep(StdDuration::from_secs(1)).await;
                }
                if ok_chunks == chunks.len() {
                    let merged = raw.join(format!("ventas_{}.csv", m.label));
                    let mut py = String::from("import csv; w=csv.writer(open(r'");
                    py.push_str(&merged.display().to_string());
                    py.push_str("','w',newline='',encoding='utf-8')); first=True\n");
                    for h in &chunks {
                        let part_csv = raw.join(format!("ventas_{}.csv", h.label));
                        py.push_str(&format!("for f in [r'{}']:\n r=csv.reader(open(f,encoding='utf-8'))\n h=next(r)\n if first: w.writerow(h); first=False\n for row in r: w.writerow(row)\n", part_csv.display()));
                    }
                    let _ = std::process::Command::new(
                        if std::process::Command::new("python3")
                            .arg("--version")
                            .output()
                            .is_ok()
                        {
                            "python3"
                        } else {
                            "python"
                        },
                    )
                    .arg("-c")
                    .arg(&py)
                    .output();
                    recovered = true;
                    info!("  granular {} partes OK {}", parts, m.label);
                    break;
                }
            }
            if !recovered {
                warn!("  {} granular 10 fallo, probando dia-a-dia", m.label);
                let mut cur = m.start;
                let mut day_ok = 0;
                let mut day_total = 0;
                while cur <= m.end {
                    day_total += 1;
                    let label = cur.format("%Y-%m-%d").to_string();
                    let day_range = MonthRange {
                        start: cur,
                        end: cur,
                        label: label.clone(),
                    };
                    let mut d_ok = false;
                    for ha in 1..=2 {
                        let browser_d = match Browser::new(
                            LaunchOptionsBuilder::default()
                                .headless(true)
                                .build()
                                .map_err(|e| anyhow!("Launch: {}", e))?,
                        )
                        .map_err(|e| anyhow!("{:?}", e))
                        {
                            Ok(b) => b,
                            Err(e) => {
                                warn!("  day {} browser fail: {}", label, e);
                                sleep(StdDuration::from_secs(3)).await;
                                continue;
                            }
                        };
                        match capture_month(&browser_d, &day_range, &raw).await {
                            Ok(xls_d) => {
                                let csv_d = raw.join(format!("ventas_{}.csv", label));
                                let py_d = format!("import xlrd, csv; wb=xlrd.open_workbook(r'{}'); sh=wb.sheet_by_index(0); w=csv.writer(open(r'{}','w',newline='',encoding='utf-8')); [w.writerow([sh.cell_value(r,c) for c in range(sh.ncols)]) for r in range(sh.nrows)]", xls_d.display(), csv_d.display());
                                let _ = std::process::Command::new(
                                    if std::process::Command::new("python3")
                                        .arg("--version")
                                        .output()
                                        .is_ok()
                                    {
                                        "python3"
                                    } else {
                                        "python"
                                    },
                                )
                                .arg("-c")
                                .arg(&py_d)
                                .output();
                                d_ok = true;
                                break;
                            }
                            Err(_) => {}
                        }
                        sleep(StdDuration::from_secs(2)).await;
                    }
                    if d_ok {
                        day_ok += 1;
                    }
                    cur = cur + Duration::days(1);
                    sleep(StdDuration::from_secs(1)).await;
                }
                if day_ok == day_total && day_total > 0 {
                    let merged = raw.join(format!("ventas_{}.csv", m.label));
                    let mut py2 = String::from("import csv; w=csv.writer(open(r'");
                    py2.push_str(&merged.display().to_string());
                    py2.push_str("','w',newline='',encoding='utf-8')); first=True\n");
                    let mut cur2 = m.start;
                    while cur2 <= m.end {
                        let f = raw.join(format!("ventas_{}.csv", cur2.format("%Y-%m-%d")));
                        py2.push_str(&format!("try:\n r=csv.reader(open(r'{}',encoding='utf-8'))\n h=next(r)\n if first: w.writerow(h); first=False\n for row in r: w.writerow(row)\nexcept: pass\n", f.display()));
                        cur2 = cur2 + Duration::days(1);
                    }
                    let _ = std::process::Command::new(
                        if std::process::Command::new("python3")
                            .arg("--version")
                            .output()
                            .is_ok()
                        {
                            "python3"
                        } else {
                            "python"
                        },
                    )
                    .arg("-c")
                    .arg(&py2)
                    .output();
                    if merged.exists() {
                        recovered = true;
                        info!("  dia-a-dia OK {} ({}/{})", m.label, day_ok, day_total);
                    }
                }
            }
            if recovered {
                ok.push(m.label.clone());
            } else {
                fail.push((
                    m.label.clone(),
                    "3 intentos + granular 2/4/10/dia fallo".to_string(),
                ));
            }
        } else {
            ok.push(m.label.clone());
        }
        if idx + 1 < pending.len() {
            sleep(StdDuration::from_secs(SLEEP_BETWEEN_MONTHS)).await;
        }
    }

    info!("DONE {}/{} ok {} fail", ok.len(), pending.len(), fail.len());
    let s = serde_json::json!({"ok": ok, "fail": fail, "total": pending.len()});
    let fname = if daily {
        "summary_daily.json"
    } else {
        "summary_history.json"
    };
    std::fs::write(raw.join(fname), serde_json::to_string_pretty(&s)?)?;

    if !ok.is_empty() && !daily {
        let pool = init_pool().await?;
        for label in &ok {
            let csv = raw.join(format!("ventas_{}.csv", label));
            if csv.exists() {
                if let Ok(v) = parse_export_csv(&csv) {
                    let _ = insert_ventas(&pool, &v).await;
                    info!("  normalized {} -> {}", label, v.len());
                }
            }
        }
        let _ = dedup_ventas(&pool).await;
    }
    if daily && !ok.is_empty() {
        let pool = init_pool().await?;
        for label in &ok {
            let csv = raw.join(format!("ventas_{}.csv", label));
            if csv.exists() {
                if let Ok(v) = parse_export_csv(&csv) {
                    let _ = insert_ventas(&pool, &v).await;
                }
            }
        }
        let _ = dedup_ventas(&pool).await;
        info!("daily dedup done");
    }
    Ok(())
}

fn gen_month_range(sy: i32, sm: u32, ey: i32, em: u32) -> Vec<MonthRange> {
    let mut out = Vec::new();
    let (mut y, mut m) = (sy, sm);
    while y < ey || (y == ey && m <= em) {
        let s = NaiveDate::from_ymd_opt(y, m, 1).unwrap();
        let (ey2, em2) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        let e = NaiveDate::from_ymd_opt(ey2, em2, 1).unwrap() - Duration::days(1);
        out.push(MonthRange {
            start: s,
            end: e,
            label: format!("{}-{:02}", y, m),
        });
        if m == 12 {
            y += 1;
            m = 1;
        } else {
            m += 1;
        }
    }
    out
}

fn split_month_n(m: &MonthRange, parts: usize) -> Vec<MonthRange> {
    let mut out = Vec::new();
    let mut cur = m.start;
    for i in 0..parts {
        if cur > m.end {
            break;
        }
        let remaining = parts - i;
        let remaining_days = (m.end - cur).num_days() + 1;
        let chunk = (remaining_days + remaining as i64 - 1) / remaining as i64;
        let mut end = cur + Duration::days(chunk - 1);
        if end > m.end {
            end = m.end;
        }
        out.push(MonthRange {
            start: cur,
            end,
            label: format!("{}-p{}_{}", m.label, parts, i + 1),
        });
        cur = end + Duration::days(1);
    }
    out
}
