// Cliente HTTP puro para el intranet CIPSA — reemplaza a Chrome como transporte primario.
// Flujo por descarga (descubierto empiricamente): el servidor fija el rango de fechas con el
// ultimo GET (render del grid), y el POST del boton Exportar devuelve el XLS de ESE rango.
// Reusar VIEWSTATE entre rangos distintos devuelve datos obsoletos: cada chunk requiere su GET.
use anyhow::{anyhow, Result};
use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

use crate::browser::captor::MonthRange;
use crate::config::{effective_intranet_pass, effective_intranet_user};

const BASE: &str = "http://intranet.cipsa.com.pe";
const LOGIN_URL: &str = "http://intranet.cipsa.com.pe/intranetcipsa/login.aspx";
const BTN_EXPORT: &str = "ctl00$ContentPlaceHolder1$btnExportar";

pub struct CipsaHttp {
    client: reqwest::Client,
}

/// Extrae hidden fields ASP.NET (__VIEWSTATE etc) del atributo value=
fn parse_hidden_fields(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for m in html.match_indices("<input") {
        let rest = &html[m.0..(m.0 + html[m.0..].len().min(200_000))];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let name = tag
            .find("name=\"")
            .and_then(|i| {
                let s = &tag[i + 6..];
                s.find('"').map(|j| &s[..j])
            })
            .unwrap_or("");
        if !name.starts_with("__") {
            continue;
        }
        let val = tag
            .find("value=\"")
            .and_then(|i| {
                let s = &tag[i + 7..];
                s.find('"').map(|j| &s[..j])
            })
            .unwrap_or("");
        out.push((name.to_string(), val.to_string()));
    }
    out
}

fn exp_url(df: &str, dt: &str, art_i: &str, art_f: &str, accion: &str) -> String {
    format!(
        "{BASE}/ESTADISTICASVENTAS/Estadistica11.aspx?valueCli=&valueVend=&valueVend2=&valueCli2=\
         &valueSucI=&valueSucF=&accion={accion}&valueArtI={art_i}&valueArtF={art_f}&valueAlmI=&valueAlmF=\
         &valueFI={df}&valueFF={dt}&valueGrat=1&valueDocs=01F%2c+01B%2c+01NCR%2c+01NDB"
    )
}

impl CipsaHttp {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(480))
            .connect_timeout(Duration::from_secs(15))
            .tcp_keepalive(Duration::from_secs(20))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;
        Ok(Self { client })
    }

    /// Login HTTP puro: GET login.aspx -> POST txtnombre/txtpass/Button1 (~0.1s vs ~10s Chrome)
    pub async fn login(&self) -> Result<()> {
        info!("  [1] Login HTTP puro");
        let html = self.client.get(LOGIN_URL).send().await?.text().await?;
        let mut form: Vec<(String, String)> = parse_hidden_fields(&html);
        if form.is_empty() {
            return Err(anyhow!("login.aspx sin hidden fields (server cambio?)"));
        }
        form.push(("txtnombre".into(), effective_intranet_user()));
        form.push(("txtpass".into(), effective_intranet_pass()));
        form.push(("Button1".into(), "Aceptar".into()));
        let resp = self
            .client
            .post(LOGIN_URL)
            .form(&form)
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        let final_url = resp.url().to_string();
        if final_url.contains("login") {
            // El POST puede volver al mismo URL con error en el HTML
            let body = resp.text().await?;
            if body.contains("txtnombre") && body.contains("txtpass") {
                return Err(anyhow!("Login HTTP fallo (credenciales rechazadas)"));
            }
        }
        info!("  Login HTTP OK");
        Ok(())
    }

    async fn get_and_post_export(
        &self,
        url: &str,
        out_path: &Path,
    ) -> Result<PathBufAlias> {
        use std::path::PathBuf;
        // Abort cooperativo: si el usuario pidio detener, cortar antes de iniciar red
        if crate::capture::CAPTURE_ABORT_GLOBAL.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow!("abortado por usuario"));
        }
        // 1) GET fija el rango en la sesion del server y renderiza __VIEWSTATE fresco
        // (dias pesados ~12k filas tardan >200s en renderizar el grid)
        // Cache-busting: no-cache headers + _t param para que no traiga la misma info
        let url_nocache = if url.contains('?') { format!("{url}&_t={}", crate::capture_state::now_secs()) } else { format!("{url}?_t={}", crate::capture_state::now_secs()) };
        let html = self
            .client
            .get(&url_nocache)
            .timeout(Duration::from_secs(420))
            .header("Cache-Control", "no-cache, no-store, must-revalidate")
            .header("Pragma", "no-cache")
            .header("Expires", "0")
            .send()
            .await?
            .text()
            .await?;
        let fields = parse_hidden_fields(&html);
        if fields.is_empty() || url_is_login(&html) {
            return Err(anyhow!("sesion perdida (no hay __VIEWSTATE)"));
        }
        // 2) POST Exportar sobre esa misma pagina — chequear abort despues del GET largo
        if crate::capture::CAPTURE_ABORT_GLOBAL.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow!("abortado por usuario"));
        }
        let mut form = fields.clone();
        form.push((BTN_EXPORT.into(), "Exportar a ".into()));
        let resp = self
            .client
            .post(&url_nocache)
            .form(&form)
            .timeout(Duration::from_secs(420))
            .header("Referer", &url_nocache)
            .header("Cache-Control", "no-cache, no-store, must-revalidate")
            .header("Pragma", "no-cache")
            .send()
            .await?;
        let status = resp.status();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() {
            return Err(anyhow!("export status={status} ct={ct}"));
        }
        let bytes = resp.bytes().await?;
        validate_xls(&bytes, &ct)?;
        std::fs::write(out_path, &bytes)?;
        info!("  Downloaded {} bytes (ct={}) -> {}", bytes.len(), ct, out_path.display());
        Ok(out_path.to_path_buf())
    }
}

type PathBufAlias = std::path::PathBuf;

fn url_is_login(html: &str) -> bool {
    // Pagina de login contiene el input txtnombre; el reporte no
    html.contains("id=\"txtnombre\"") || html.contains("id='txtnombre'")
}

fn validate_xls(bytes: &[u8], ct: &str) -> Result<()> {
    if bytes.len() < 512 {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).to_lowercase();
        if head.contains("<html") || head.contains("error") || head.contains("exception") {
            return Err(anyhow!(
                "export devolvio HTML de error ({} bytes): {}",
                bytes.len(),
                head.chars().take(150).collect::<String>()
            ));
        }
    }
    if bytes.len() >= 4 {
        let is_ole = bytes[0] == 0xD0 && bytes[1] == 0xCF && bytes[2] == 0x11 && bytes[3] == 0xE0;
        let is_zip = bytes[0] == 0x50 && bytes[1] == 0x4B;
        let is_html = bytes[0] == 0x3C;
        if is_html && !is_ole && !is_zip {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_string();
            return Err(anyhow!(
                "export devolvio HTML en vez de XLS (ct={ct}, {} bytes): {}",
                bytes.len(),
                head.chars().take(120).collect::<String>()
            ));
        }
        if !is_ole && !is_zip && ct.contains("text/html") && bytes.len() < 2048 {
            return Err(anyhow!("respuesta sospechosa text/html {} bytes", bytes.len()));
        }
    }
    Ok(())
}

impl CipsaHttp {
    fn fname(month: &MonthRange, suffix: &str) -> String {
        if suffix.is_empty() {
            format!("ventas_{}.xls", month.label)
        } else {
            format!("ventas_{}{}.xls", month.label, suffix)
        }
    }

    pub async fn download_with_art(
        &self,
        month: &MonthRange,
        output_dir: &Path,
        art_i: &str,
        art_f: &str,
        label_suffix: &str,
    ) -> Result<std::path::PathBuf> {
        let (df, dt) = month.to_url_params();
        let url = exp_url(&df, &dt, art_i, art_f, "D0");
        let out = output_dir.join(Self::fname(month, label_suffix));
        match self.get_and_post_export(&url, &out).await {
            Ok(p) => Ok(p),
            Err(e) => {
                let es = format!("{e}");
                if es.contains("sesion perdida") {
                    warn!("  sesion perdida — re-login y reintento unico");
                    self.login().await?;
                    self.get_and_post_export(&url, &out).await
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Wrapper compat sin split Art
    pub async fn download(&self, month: &MonthRange, output_dir: &Path) -> Result<std::path::PathBuf> {
        self.download_with_art(month, output_dir, "", "", "").await
    }

    /// Split adaptativo por Articulo (2->4->8) sobre transporte HTTP.
    /// Misma estrategia que captor.rs pero sin navegacion Chrome.
    pub async fn download_day_split(&self, day: &MonthRange, output_dir: &Path) -> Result<std::path::PathBuf> {
        if let Ok(p) = self.download(day, output_dir).await {
            if std::fs::metadata(&p).map(|m| m.len() > 5000).unwrap_or(false) {
                return Ok(p);
            }
        }
        warn!("  Dia {} muy extenso — split Art adaptativo via HTTP", day.label);
        let schemes: Vec<Vec<(&str, &str)>> = vec![
            vec![("", "5ZZZZZ"), ("6", "ZZZZZ")],
            vec![("", "3ZZZZZ"), ("4", "6ZZZZZ"), ("7", "9ZZZZZ"), (":", "ZZZZZ")],
            vec![
                ("", "1ZZZZZ"), ("2", "3ZZZZZ"), ("4", "5ZZZZZ"), ("6", "7ZZZZZ"),
                ("8", "9ZZZZZ"), (":", "GZZZZZ"), ("H", "NZZZZZ"), ("O", "ZZZZZ"),
            ],
        ];
        for (si, buckets) in schemes.iter().enumerate() {
            warn!("  esquema {}/{}: {} partes", si + 1, schemes.len(), buckets.len());
            let mut part_csvs: Vec<std::path::PathBuf> = Vec::new();
            let mut ok_all = true;
            for (bi, (a_i, a_f)) in buckets.iter().enumerate() {
                let suffix = format!("-s{}_{}", si + 1, bi + 1);
                match self.download_with_art(day, output_dir, a_i, a_f, &suffix).await {
                    Ok(xls_p) => {
                        let csv_p = xls_p.with_extension("csv");
                        if crate::processor::xls::xls_to_csv(&xls_p, &csv_p).is_err()
                            || !csv_p.exists()
                            || std::fs::metadata(&csv_p).map(|m| m.len() < 500).unwrap_or(true)
                        {
                            warn!("  bucket {} xls_to_csv fallo", bi + 1);
                            ok_all = false;
                            break;
                        }
                        part_csvs.push(csv_p);
                    }
                    Err(e) => {
                        warn!("  bucket {} fail: {}", bi + 1, e);
                        ok_all = false;
                        break;
                    }
                }
            }
            if ok_all && !part_csvs.is_empty() {
                let merged_csv = output_dir.join(format!("ventas_{}.csv", day.label));
                // Merge en Rust puro (sin python)
                if let Err(e) = crate::capture::merge_csvs(&part_csvs, &merged_csv) {
                    warn!("  split merge rust failed: {e:#}");
                }
                if merged_csv.exists()
                    && std::fs::metadata(&merged_csv).map(|m| m.len() > 1000).unwrap_or(false)
                {
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

    /// Fallback HTML: GET accion=L0 trae el grid ya renderizado; se convierte tabla -> CSV.
    /// Parser Rust puro (sin python): extrae la tabla con mas <tr>.
    pub async fn scrape_html(&self, month: &MonthRange, output_dir: &Path) -> Result<std::path::PathBuf> {
        let (df, dt) = month.to_url_params();
        let url = exp_url(&df, &dt, "", "", "L0");
        let html = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(300))
            .send()
            .await?
            .text()
            .await?;
        if html.len() < 5000 || url_is_login(&html) {
            return Err(anyhow!("L0 sin grid ({})", html.len()));
        }
        // Parser tabla HTML minimalista: busca <table>, cuenta <tr>, extrae celdas de la mayor
        let table_csv = extract_largest_table_html(&html);
        if table_csv.len() < 100 {
            return Err(anyhow!("L0 tabla vacia ({} bytes)", table_csv.len()));
        }
        let csv_out = output_dir.join(format!("ventas_{}.csv", month.label));
        std::fs::write(&csv_out, &table_csv)?;
        let nrows = table_csv.lines().count();
        if nrows < 50 {
            return Err(anyhow!("L0 tabla vacia ({nrows} filas)"));
        }
        info!("  scrape_html L0: {nrows} filas -> {}", csv_out.display());
        // XLS dummy para compat con pipeline existente
        let xls = output_dir.join(format!("ventas_{}.xls", month.label));
        let _ = std::fs::copy(&csv_out, &xls);
        Ok(xls)
    }
}

/// Extrae la tabla con mas filas del HTML y la devuelve como CSV.
/// Parser manual tolerante (sin crate html): recorre tags tr/td/th, decodifica entidades basicas.
fn extract_largest_table_html(html: &str) -> String {
    fn decode_entities(s: &str) -> String {
        s.replace("&nbsp;", " ")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
    }
    let lower = html.to_lowercase();
    let mut best_rows: Vec<Vec<String>> = Vec::new();
    // Iterar todas las <table>
    let mut pos = 0usize;
    while let Some(t_start) = lower[pos..].find("<table") {
        let abs_start = pos + t_start;
        let t_end = match lower[abs_start..].find("</table>") {
            Some(e) => abs_start + e,
            None => break,
        };
        let table_html = &lower[abs_start..t_end];
        // Extraer filas
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut rpos = 0usize;
        while let Some(r_start) = table_html[rpos..].find("<tr") {
            let r_abs = rpos + r_start;
            let r_end = match table_html[r_abs..].find("</tr>") {
                Some(e) => r_abs + e,
                None => break,
            };
            let row_html = &table_html[r_abs..r_end];
            let mut cells: Vec<String> = Vec::new();
            let mut cpos = 0usize;
            while let Some(c_start) = row_html[cpos..].find("<td").or_else(|| row_html[cpos..].find("<th")) {
                let c_abs = cpos + c_start;
                // contenido entre > y </td|th>
                let open_end = match row_html[c_abs..].find('>') {
                    Some(e) => c_abs + e + 1,
                    None => break,
                };
                let close_tag = if &row_html[c_abs..c_abs+3] == "<td" { "</td>" } else { "</th>" };
                let close_pos = match row_html[open_end..].find(close_tag) {
                    Some(e) => open_end + e,
                    None => break,
                };
                let mut text: String = decode_entities(row_html[open_end..close_pos].trim());
                // colapsar whitespace/newlines
                text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                // quitar comas que romperian el CSV
                text = text.replace(',', " ");
                cells.push(text);
                cpos = close_pos + close_tag.len();
            }
            if !cells.is_empty() {
                rows.push(cells);
            }
            rpos = r_end + 5;
        }
        if rows.len() > best_rows.len() {
            best_rows = rows;
        }
        pos = t_end;
        if best_rows.len() > 50_000 { break; } // suficiente
    }
    // Serializar a CSV
    let mut out = String::with_capacity(best_rows.len() * 120);
    for row in &best_rows {
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}
