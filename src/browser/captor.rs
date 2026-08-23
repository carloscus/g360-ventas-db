use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};
use headless_chrome::Browser;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

use crate::config::{intranet_password, intranet_username, LOGIN_URL};

#[derive(Debug, Clone)]
pub struct MonthRange {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub label: String,
}

impl MonthRange {
    pub fn to_url_params(&self) -> (String, String) {
        (
            self.start.format("%d/%m/%Y").to_string(),
            self.end.format("%d/%m/%Y").to_string(),
        )
    }
}

pub fn generate_month_ranges(count: u32) -> Vec<MonthRange> {
    let now = chrono::Local::now().naive_local();
    let (cur_y, cur_m) = (now.year(), now.month());
    (0..count)
        .map(|i| {
            let mb = i + 1;
            let (y, m) = if cur_m > mb {
                (cur_y, cur_m - mb)
            } else {
                (cur_y - 1, cur_m + 12 - mb)
            };
            let start = NaiveDate::from_ymd_opt(y, m, 1).unwrap();
            let (ey, em) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            let end = NaiveDate::from_ymd_opt(ey, em, 1).unwrap() - chrono::Duration::days(1);
            MonthRange {
                start,
                end,
                label: format!("{}-{:02}", y, m),
            }
        })
        .collect()
}

pub fn generate_day_range(date: NaiveDate) -> MonthRange {
    MonthRange {
        start: date,
        end: date,
        label: date.format("%Y-%m-%d").to_string(),
    }
}

fn js(tab: &headless_chrome::Tab, code: &str) -> String {
    tab.evaluate(code, false)
        .ok()
        .and_then(|r| r.value)
        .map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else if let Some(n) = v.as_f64() {
                (n as i64).to_string()
            } else if let Some(b) = v.as_bool() {
                b.to_string()
            } else {
                v.to_string()
            }
        })
        .unwrap_or_default()
}

/// Download XLS export via reqwest using session from Chrome
async fn download_xls(
    tab: &headless_chrome::Tab,
    month: &MonthRange,
    output_dir: &Path,
) -> Result<PathBuf> {
    let (df, dt) = month.to_url_params();

    // Get cookies from Chrome via CDP (includes HttpOnly)
    let cookies = js(tab, "document.cookie");

    // Get form fields
    let vs_full = js(tab, "document.getElementById('__VIEWSTATE').value");
    let ev_full = js(tab, "document.getElementById('__EVENTVALIDATION').value");
    let vsg = js(tab, "document.getElementById('__VIEWSTATEGENERATOR').value");

    // Build export URL
    let export_url = format!(
        "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?\
         valueCli=&valueVend=&valueVend2=&valueCli2=&\
         valueSucI=&valueSucF=&accion=D0&\
         valueArtI=&valueArtF=&valueAlmI=&valueAlmF=&\
         valueFI={}&valueFF={}&valueGrat=1&\
         valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB",
        df, dt
    );

    let cookie_header: String = cookies
        .split(';')
        .map(|c| c.trim())
        .collect::<Vec<_>>()
        .join("; ");

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .danger_accept_invalid_certs(true)
        .build()?;

    let resp = client
        .post(&export_url)
        .header("Cookie", &cookie_header)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .form(&[
            ("__VIEWSTATE", vs_full.as_str()),
            ("__VIEWSTATEGENERATOR", vsg.as_str()),
            ("__EVENTVALIDATION", ev_full.as_str()),
            ("ctl00$ContentPlaceHolder1$btnExportar", "Exportar a "),
        ])
        .send()
        .await
        .map_err(|e| anyhow!("reqwest: {:?}", e))?;

    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if !status.is_success() {
        return Err(anyhow!("Export failed: status={}, ct={}", status, ct));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow!("read body: {:?}", e))?;
    let out_path = output_dir.join(format!("ventas_{}.xls", month.label));
    std::fs::write(&out_path, &bytes).map_err(|e| anyhow!("write: {:?}", e))?;

    info!(
        "  Downloaded {} bytes ({}) -> {}",
        bytes.len(),
        ct,
        out_path.display()
    );
    Ok(out_path)
}

pub async fn capture_month(
    browser: &Browser,
    month: &MonthRange,
    output_dir: &Path,
) -> Result<PathBuf> {
    info!(
        "=== CAPTURE {} ({} -> {}) ===",
        month.label, month.start, month.end
    );

    let tab = browser.new_tab().map_err(|e| anyhow!("{:?}", e))?;

    // 1. Login
    info!("  [1] Login");
    tab.navigate_to(LOGIN_URL).map_err(|e| anyhow!("{:?}", e))?;
    std::thread::sleep(Duration::from_secs(3));
    js(
        &tab,
        &format!(
            "document.getElementById('txtnombre').value='{}'",
            intranet_username()
        ),
    );
    tab.press_key("Tab").map_err(|e| anyhow!("{:?}", e))?;
    std::thread::sleep(Duration::from_millis(300));
    js(
        &tab,
        &format!(
            "document.getElementById('txtpass').value='{}'",
            intranet_password()
        ),
    );
    tab.press_key("Enter").map_err(|e| anyhow!("{:?}", e))?;
    std::thread::sleep(Duration::from_secs(3));
    if tab.get_url().contains("login") {
        return Err(anyhow!("Login failed"));
    }
    info!("  Login OK");

    // 2. Navigate to results page
    let report_url = format!(
        "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?\
         valueCli=&valueVend=&valueVend2=&valueCli2=&\
         valueSucI=&valueSucF=&accion=D0&\
         valueArtI=&valueArtF=&valueAlmI=&valueAlmF=&\
         valueFI={}&valueFF={}&valueGrat=1&\
         valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB",
        month.to_url_params().0,
        month.to_url_params().1
    );
    info!("  [2] Navigate to report");
    tab.navigate_to(&report_url)
        .map_err(|e| anyhow!("{:?}", e))?;
    std::thread::sleep(Duration::from_secs(10));

    // 3. Download XLS via reqwest
    info!("  [3] Download XLS via reqwest");
    let xls_path = download_xls(&tab, month, output_dir).await?;

    info!("=== DONE ===");
    Ok(xls_path)
}
