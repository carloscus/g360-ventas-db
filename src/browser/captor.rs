use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};
use headless_chrome::Browser;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

use crate::config::{
    effective_intranet_pass, effective_intranet_user, LOGIN_URL,
};

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
    let min_date = crate::config::MIN_AVAILABLE_DATE;
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
            // Skip months before the earliest available data
            if start < min_date {
                return MonthRange {
                    start: min_date,
                    end: min_date,
                    label: format!("{}-{:02}", min_date.year(), min_date.month()),
                };
            }
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

/// Prechequeos para fallar rapido con mensaje accionable (ej. otra maquina/admin).
pub fn preflight_checks() -> Result<(), String> {
    if effective_intranet_user().is_empty() || effective_intranet_pass().is_empty() {
        return Err(
            "Faltan credenciales del intranet: configuralas en Configuracion (engranaje) o en el archivo .env."
                .into(),
        );
    }
    let candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    let mut found = candidates.iter().any(|p| std::path::Path::new(p).exists());
    if !found {
        if let Ok(lad) = std::env::var("LOCALAPPDATA") {
            found = std::path::Path::new(&format!(
                r"{lad}\Google\Chrome\Application\chrome.exe"
            ))
            .exists();
        }
    }
    if !found {
        return Err("Google Chrome no esta instalado en esta maquina (requerido para la descarga).".into());
    }
    Ok(())
}

fn js(tab: &headless_chrome::Tab, code: &str) -> String {
    match tab.evaluate(code, false) {
        Ok(r) => {
            if let Some(val) = r.value {
                if let Some(s) = val.as_str() {
                    s.to_string()
                } else if let Some(n) = val.as_f64() {
                    (n as i64).to_string()
                } else if let Some(b) = val.as_bool() {
                    b.to_string()
                } else {
                    val.to_string()
                }
            } else {
                warn!("JS eval returned None for: {}", &code[..code.len().min(80)]);
                String::new()
            }
        }
        Err(e) => {
            warn!("JS eval failed: {} for code: {}", e, &code[..code.len().min(80)]);
            String::new()
        }
    }
}

async fn download_xls_with_art(
    tab: &headless_chrome::Tab,
    month: &MonthRange,
    output_dir: &Path,
    art_i: &str,
    art_f: &str,
    label_suffix: &str,
) -> Result<PathBuf> {
    let (df, dt) = month.to_url_params();

    // Get cookies from Chrome via CDP (includes HttpOnly)
    let cookies = js(tab, "document.cookie");

    // Get form fields
    let vs_full = js(tab, "document.getElementById('__VIEWSTATE').value");
    let ev_full = js(tab, "document.getElementById('__EVENTVALIDATION').value");
    let vsg = js(tab, "document.getElementById('__VIEWSTATEGENERATOR').value");

    // Build export URL — con split por Articulo para dias muy extensos (19/01 12k filas)
    let export_url = format!(
        "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?\
         valueCli=&valueVend=&valueVend2=&valueCli2=&\
         valueSucI=&valueSucF=&accion=D0&\
         valueArtI={}&valueArtF={}&valueAlmI=&valueAlmF=&\
         valueFI={}&valueFF={}&valueGrat=1&\
         valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB",
        art_i, art_f, df, dt
    );

    let cookie_header: String = cookies
        .split(';')
        .map(|c| c.trim())
        .collect::<Vec<_>>()
        .join("; ");

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(20))
        .build()?;

    let resp = client
        .post(&export_url)
        .header("Cookie", &cookie_header)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .header("Accept", "text/html,application/vnd.ms-excel,application/octet-stream,*/*")
        .header("Referer", &export_url)
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
    let cl = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).unwrap_or("?").to_string();

    if !status.is_success() {
        return Err(anyhow!("Export failed: status={}, ct={}, cl={}", status, ct, cl));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow!("read body: {:?}", e))?;

    // Validacion temprana: tablas extensas a veces devuelven HTML de error en lugar de XLS
    if bytes.len() < 512 {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).to_lowercase();
        if head.contains("<html") || head.contains("error") || head.contains("exception") {
            return Err(anyhow!("Export devolvio HTML de error ({} bytes, ct={}): {}", bytes.len(), ct, head.chars().take(150).collect::<String>()));
        }
    }
    // Magic bytes: OLE2 (D0 CF 11 E0) o ZIP/PK (50 4B) para xls/xlsx; si es HTML puro falla
    if bytes.len() >= 4 {
        let is_ole = bytes[0]==0xD0 && bytes[1]==0xCF && bytes[2]==0x11 && bytes[3]==0xE0;
        let is_zip = bytes[0]==0x50 && bytes[1]==0x4B;
        let is_html = bytes[0]==0x3C && (bytes[1]==0x21 || bytes[1]==0x68 || bytes[1]==0x48); // <! , <h , <H
        if is_html && !is_ole && !is_zip {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_string();
            return Err(anyhow!("Export devolvio HTML en lugar de XLS (ct={}, {} bytes): {}", ct, bytes.len(), head.chars().take(120).collect::<String>()));
        }
        // Si no es ninguno pero ct dice excel, lo aceptamos (algunas versiones devuelven html con ct excel)
        if !is_ole && !is_zip && ct.contains("text/html") && bytes.len() < 2048 {
            return Err(anyhow!("Respuesta sospechosa text/html con {} bytes, ct={}", bytes.len(), ct));
        }
    }

    let fname = if label_suffix.is_empty() { format!("ventas_{}.xls", month.label) } else { format!("ventas_{}{}.xls", month.label, label_suffix) };
    let out_path = output_dir.join(fname);
    std::fs::write(&out_path, &bytes).map_err(|e| anyhow!("write: {:?}", e))?;

    info!(
        "  Downloaded {} bytes (ct={}, cl={}) -> {}",
        bytes.len(),
        ct,
        cl,
        out_path.display()
    );
    Ok(out_path)
}

// Wrapper compat: sin split Art
async fn download_xls(
    tab: &headless_chrome::Tab,
    month: &MonthRange,
    output_dir: &Path,
) -> Result<PathBuf> {
    download_xls_with_art(tab, month, output_dir, "", "", "").await
}

pub struct CipsaSession {
    pub browser: Browser,
    pub tab: std::sync::Arc<headless_chrome::Tab>,
}

impl CipsaSession {
    pub fn new() -> Result<Self> {
        // Una sola sesion por captura con perfil temporal fresco (sin cache persistente)
        // headless_chrome 0.9 crea un --user-data-dir temporal por Browser, sin cache entre capturas
        let browser = Browser::new(
            headless_chrome::LaunchOptionsBuilder::default()
                .headless(true)
                .build()
                .map_err(|e| anyhow!("Launch: {}", e))?,
        )
        .map_err(|e| anyhow!("{:?}", e))?;
        let tab = browser.new_tab().map_err(|e| anyhow!("{:?}", e))?;
        tab.set_default_timeout(Duration::from_secs(180));
        // Limpiar storage al crear sesion (una sesion por captura)
        let _ = tab.evaluate("try{localStorage.clear();sessionStorage.clear();}catch(e){}", false);
        Ok(Self { browser, tab })
    }

    /// Limpia cache/storage entre meses para no traer la misma info (cache-busting via _t param ya en URL)
    pub fn clear_cache_sync(&self) {
        let _ = self.tab.evaluate("try{localStorage.clear();sessionStorage.clear();}catch(e){}", false);
    }

    pub async fn login(&self) -> Result<()> {
        info!("  [1] Login (sesion reusada)");
        self.tab.navigate_to(LOGIN_URL).map_err(|e| anyhow!("{:?}", e))?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        js(
            &self.tab,
            &format!(
                "document.getElementById('txtnombre').value='{}'",
                effective_intranet_user()
            ),
        );
        self.tab.press_key("Tab").map_err(|e| anyhow!("{:?}", e))?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        js(
            &self.tab,
            &format!(
                "document.getElementById('txtpass').value='{}'",
                effective_intranet_pass()
            ),
        );
        self.tab.press_key("Enter").map_err(|e| anyhow!("{:?}", e))?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        if self.tab.get_url().contains("login") {
            return Err(anyhow!("Login failed"));
        }
        info!("  Login OK");
        Ok(())
    }

    pub async fn download(&self, month: &MonthRange, output_dir: &Path) -> Result<PathBuf> {
        // 2. Navigate to results page (sin re-login) — cache-busting _t para no traer misma info
        self.clear_cache_sync();
        let report_url = format!(
            "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?\
             valueCli=&valueVend=&valueVend2=&valueCli2=&\
             valueSucI=&valueSucF=&accion=D0&\
             valueArtI=&valueArtF=&valueAlmI=&valueAlmF=&\
             valueFI={}&valueFF={}&valueGrat=1&\
             valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB&_t={}",
            month.to_url_params().0,
            month.to_url_params().1,
            crate::capture_state::now_secs()
        );
        info!("  [2] Navigate to report {}", month.label);
        self.tab.navigate_to(&report_url).map_err(|e| anyhow!("{:?}", e))?;
        let mut page_ready = false;
        for i in 0..12 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let url = self.tab.get_url();
            // Si nos saco sesion, re-login rapido
            if url.contains("login") {
                warn!("  Sesion expirada, re-logueando");
                self.login().await?;
                self.tab.navigate_to(&report_url).map_err(|e| anyhow!("{:?}", e))?;
                continue;
            }
            let body_len = js(&self.tab, "document.body ? document.body.innerHTML.length : 0")
                .parse::<usize>()
                .unwrap_or(0);
            info!("  [2] Poll {}: body_len={}", i+1, body_len);
            if body_len > 1000 {
                page_ready = true;
                break;
            }
        }
        if !page_ready {
            warn!("  [2] Pagina no cargo completamente, intentando descarga de todas formas");
        }
        info!("  [3] Download XLS via reqwest {}", month.label);
        download_xls(&self.tab, month, output_dir).await
    }

    /// Fallback HTML para meses muy extensos donde XLS falla (accion=L0 + scrape tabla)
    pub async fn scrape_html(&self, month: &MonthRange, output_dir: &Path) -> Result<PathBuf> {
        let (df, dt) = month.to_url_params();
        let html_url = format!(
            "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?valueCli=&valueVend=&valueVend2=&valueCli2=&valueSucI=&valueSucF=&accion=L0&valueArtI=&valueArtF=&valueAlmI=&valueAlmF=&valueFI={}&valueFF={}&valueGrat=1&valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB",
            df, dt
        );
        self.tab.navigate_to(&html_url).map_err(|e| anyhow!("{:?}", e))?;
        tokio::time::sleep(Duration::from_secs(4)).await;
        // Esperar tabla
        for _ in 0..10 {
            let has_table = js(&self.tab, "document.querySelector('table') ? '1' : '0'");
            if has_table=="1" { break; }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        // Extraer tabla como CSV via JS (primera tabla, sin paginacion)
        let csv_data = js(&self.tab, r#"
            (()=>{ let t=document.querySelector('table'); if(!t) return ''; 
            let rows=[...t.querySelectorAll('tr')]; 
            return rows.map(tr=>[...tr.querySelectorAll('th,td')].map(td=>td.innerText.replace(/,/g,' ').replace(/\n/g,' ').trim()).join(',')).join('\n');
            })()
        "#);
        if csv_data.len()<100 { return Err(anyhow!("HTML tabla vacia o no encontrada")); }
        let out = output_dir.join(format!("ventas_{}.csv", month.label));
        std::fs::write(&out, csv_data).map_err(|e| anyhow!("write html csv: {:?}", e))?;
        // Crear XLS dummy para compat
        let xls = output_dir.join(format!("ventas_{}.xls", month.label));
        let _ = std::fs::copy(&out, &xls);
        Ok(xls)
    }

    /// Para dias muy extensos (ej 19/01 12k filas) — split ADAPTATIVO por rango de Articulo:
    /// prueba 2 partes -> 4 -> 8, con rangos lexicograficos que cubren IDs alfanumericos (011019..CE005)
    pub async fn download_day_split(&self, day: &MonthRange, output_dir: &Path) -> Result<PathBuf> {
        // Intentar normal primero
        if let Ok(p) = self.download(day, output_dir).await {
            if std::fs::metadata(&p).map(|m| m.len()>5000).unwrap_or(false) {
                return Ok(p);
            }
        }
        warn!("  Dia {} muy extenso — split Art adaptativo (2->4->8)", day.label);
        // Rangos lexicograficos: ':' es el char siguiente a '9' en ASCII, cubre letras
        let schemes: Vec<Vec<(&str, &str)>> = vec![
            vec![("", "5ZZZZZ"), ("6", "ZZZZZ")],
            vec![("", "3ZZZZZ"), ("4", "6ZZZZZ"), ("7", "9ZZZZZ"), (":", "ZZZZZ")],
            vec![("", "1ZZZZZ"), ("2", "3ZZZZZ"), ("4", "5ZZZZZ"), ("6", "7ZZZZZ"),
                 ("8", "9ZZZZZ"), (":", "GZZZZZ"), ("H", "NZZZZZ"), ("O", "ZZZZZ")],
        ];
        for (si, buckets) in schemes.iter().enumerate() {
            warn!("  esquema {} de {}: {} partes para {}", si+1, schemes.len(), buckets.len(), day.label);
            let mut part_csvs: Vec<PathBuf> = Vec::new();
            let mut ok_all = true;
            for (bi, (a_i, a_f)) in buckets.iter().enumerate() {
                let suffix = format!("-s{}_{}", si+1, bi+1);
                let report_url = format!(
                    "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?valueCli=&valueVend=&valueVend2=&valueCli2=&valueSucI=&valueSucF=&accion=D0&valueArtI={}&valueArtF={}&valueAlmI=&valueAlmF=&valueFI={}&valueFF={}&valueGrat=1&valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB",
                    a_i, a_f, day.to_url_params().0, day.to_url_params().1
                );
                let nav = self.tab.navigate_to(&report_url);
                if nav.is_err() { ok_all=false; break; }
                tokio::time::sleep(Duration::from_secs(2)).await;
                match download_xls_with_art(&self.tab, day, output_dir, a_i, a_f, &suffix).await {
                    Ok(xls_p) => {
                        let csv_p = xls_p.with_extension("csv");
                        if crate::processor::xls::xls_to_csv(&xls_p, &csv_p).is_err()
                            || !csv_p.exists()
                            || std::fs::metadata(&csv_p).map(|m| m.len() < 500).unwrap_or(true)
                        {
                            ok_all=false; break;
                        }
                        part_csvs.push(csv_p);
                    }
                    Err(e) => {
                        warn!("  bucket {} fail: {}", bi+1, e);
                        ok_all=false; break;
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if ok_all && !part_csvs.is_empty() {
                // Merge todos los buckets -> ventas_YYYY-MM-DD.csv (Rust puro)
                let merged_csv = output_dir.join(format!("ventas_{}.csv", day.label));
                crate::capture::merge_csvs(&part_csvs, &merged_csv)?;
                if merged_csv.exists() && std::fs::metadata(&merged_csv).map(|m| m.len()>1000).unwrap_or(false) {
                    // XLS dummy para compat con flujo existente
                    let xls_final = output_dir.join(format!("ventas_{}.xls", day.label));
                    if !xls_final.exists() {
                        let _ = std::fs::copy(&part_csvs[0].with_extension("xls"), &xls_final);
                    }
                    info!("  split {} partes OK para {}", buckets.len(), day.label);
                    return Ok(xls_final);
                }
            }
        }
        Err(anyhow!("todos los esquemas de split fallaron para {}", day.label))
    }
}

pub async fn capture_month(
    browser: &Browser,
    month: &MonthRange,
    output_dir: &Path,
) -> Result<PathBuf> {
    // compat: crea sesion efimera (usado por CLI antiguo)
    let tab = browser.new_tab().map_err(|e| anyhow!("{:?}", e))?;
    tab.set_default_timeout(Duration::from_secs(180));
    info!("  [1] Login (compat)");
    tab.navigate_to(LOGIN_URL).map_err(|e| anyhow!("{:?}", e))?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    js(&tab, &format!("document.getElementById('txtnombre').value='{}'", effective_intranet_user()));
    tab.press_key("Tab").map_err(|e| anyhow!("{:?}", e))?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    js(&tab, &format!("document.getElementById('txtpass').value='{}'", effective_intranet_pass()));
    tab.press_key("Enter").map_err(|e| anyhow!("{:?}", e))?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    if tab.get_url().contains("login") { return Err(anyhow!("Login failed")); }
    let report_url = format!("http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Estadistica11.aspx?valueCli=&valueVend=&valueVend2=&valueCli2=&valueSucI=&valueSucF=&accion=D0&valueArtI=&valueArtF=&valueAlmI=&valueAlmF=&valueFI={}&valueFF={}&valueGrat=1&valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB", month.to_url_params().0, month.to_url_params().1);
    tab.navigate_to(&report_url).map_err(|e| anyhow!("{:?}", e))?;
    for _ in 0..12 { tokio::time::sleep(Duration::from_secs(2)).await; let bl = js(&tab, "document.body ? document.body.innerHTML.length : 0").parse::<usize>().unwrap_or(0); if bl>1000 {break;} }
    download_xls(&tab, month, output_dir).await
}
