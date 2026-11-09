use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, Duration, NaiveDate};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::browser::captor::MonthRange;
use crate::config::{raw_dir, SLEEP_BETWEEN_MONTHS};
use crate::db::writer::{init_pool, insert_ventas};
use crate::processor::parser::parse_export_csv_with_cross;
use crate::capture_state::{CapturePhase, SharedProgress, now_secs};

use std::sync::atomic::{AtomicBool, Ordering};

const LOCK_FILE: &str = "capture.lock";

/// Flag global de abort — compartido con http.rs para cortar GET/POST largos.
pub static CAPTURE_ABORT_GLOBAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Inserta ventas con retry automático ante `database is locked`.
///
/// SQLite usa file-locking: si dos workers escriben al mismo tiempo, uno
/// pierde el lock. Sin retry el mes se descarta silenciosamente.
/// Con backoff exponencial (2s, 4s, 8s) el lock se libera y el insert entra.
async fn insert_with_retry(
    pool: &sqlx::SqlitePool,
    ventas: &[crate::models::Venta],
    label: &str,
) -> Result<usize> {
    const MAX_RETRY: u32 = 3;
    let mut intento = 0u32;
    loop {
        match insert_ventas(pool, ventas, false).await {
            Ok(n) => return Ok(n),
            Err(e) if e.to_string().contains("database is locked") && intento < MAX_RETRY => {
                intento += 1;
                let espera = 2u64.pow(intento); // 2s, 4s, 8s
                warn!(
                    "  DB locked en {}, retry {}/{} ({}s)...",
                    label, intento, MAX_RETRY, espera
                );
                sleep(StdDuration::from_secs(espera)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Merge de CSVs en Rust puro: header del primero una vez, resto data.
pub fn merge_csvs(parts: &[PathBuf], merged: &PathBuf) -> Result<usize> {
    use std::io::{BufRead, BufWriter, Write};
    let mut out = BufWriter::new(std::fs::File::create(merged)?);
    let mut total = 0usize;
    let mut header: Option<String> = None;
    for p in parts {
        let f = std::fs::File::open(p).with_context(|| format!("abrir {}", p.display()))?;
        let reader = std::io::BufReader::new(f);
        for (j, line) in reader.lines().enumerate() {
            let line = line?;
            // Header (primera linea): se escribe una sola vez y no cuenta como data
            if j == 0 {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(expected) = header.as_ref() {
                    if expected != &line {
                        return Err(anyhow!(
                            "headers incompatibles al combinar {}: esperado {:?}, recibido {:?}",
                            merged.display(),
                            expected,
                            line
                        ));
                    }
                } else {
                    header = Some(line.clone());
                    out.write_all(line.as_bytes())?;
                    out.write_all(b"\n")?;
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            out.write_all(line.as_bytes())?;
            out.write_all(b"\n")?;
            total += 1;
        }
    }
    out.flush()?;
    Ok(total)
}

/// RAII guard para el lock file. Se libera automaticamente al salir del scope
/// (incluso si la funcion retorna con error via `?`).
struct LockGuard {
    _file: std::fs::File,
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Cerrar handle explicito antes de borrar (Windows requiere handle cerrado)
        let _ = self._file.sync_all();
        // Drop del File directamente — Drop impl se encarga cuando el campo sale del scope
        // Reintentar borrado hasta 3 veces (Windows puede tardar en liberar)
        for _ in 0..3 {
            if std::fs::remove_file(&self.path).is_ok() { break; }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        // Fallback: si aún existe, truncar para que stale check lo borre rapido
        if self.path.exists() { let _ = std::fs::write(&self.path, ""); let _ = std::fs::remove_file(&self.path); }
    }
}

/// Adquiere el lock abriendo el archivo con handle EXCLUSIVO (share_mode=0 en Windows).
/// Si otro proceso lo mantiene abierto, falla. Si el proceso muere, el SO libera
/// el handle automaticamente -> no quedan locks huerfanos.
/// Devuelve un LockGuard que libera el lock al hacer drop.
fn acquire_lock(raw: &std::path::Path) -> Result<LockGuard> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::create_dir_all(raw)?;
    let lock_path = raw.join(LOCK_FILE);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(&lock_path)
        .map_err(|e| anyhow!("Otra captura esta en curso (lock ocupado): {e}"))?;
    let _ = f.set_len(0);
    let _ = writeln!(f, "pid={}", std::process::id());
    let _ = writeln!(f, "started_at={}", now_secs());
    let _ = f.sync_all();
    Ok(LockGuard { _file: f, path: lock_path })
}

/// Spawn a background task that periodically writes the progress state to stdout.
/// Returns a JoinHandle that can be aborted when the capture finishes.
async fn spawn_progress_writer(shared: SharedProgress) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Evita inundar el log repitiendo el mismo estado terminal (Error/Done/Idle)
        // cada 2s: solo se registra cuando cambia.
        let mut last_terminal: Option<(String, String)> = None;
        loop {
            sleep(StdDuration::from_secs(2)).await;
            let (phase, progress, message, current, started_at) = {
                let s = shared.lock().unwrap();
                (s.phase.clone(), s.progress, s.message.clone(), s.current_item.clone(), s.started_at)
            };
            let terminal = matches!(
                phase,
                CapturePhase::Error | CapturePhase::Done | CapturePhase::Idle
            );
            let snapshot = (phase.to_string(), message.clone());
            if terminal && last_terminal.as_ref() == Some(&snapshot) {
                continue;
            }
            last_terminal = Some(snapshot);
            let elapsed = if let Some(start) = started_at {
                now_secs().saturating_sub(start)
            } else {
                0
            };
            let elapsed_min = elapsed / 60;
            let elapsed_sec = elapsed % 60;
            info!(
                "[PROGRESS] phase={} progress={:.1}% msg=\"{}\" current=\"{}\" elapsed={:02}:{:02}",
                phase, progress * 100.0, message, current, elapsed_min, elapsed_sec
            );
        }
    })
}

async fn build_pending(
    _sd: &str,
    _ed: &str,
    daily: bool,
    raw: &PathBuf,
    shared: SharedProgress,
) -> Result<Vec<MonthRange>> {
    let now = chrono::Local::now().naive_local();
    let today = now.date();
    let mut pending = Vec::new();

    if daily {
        let pool = init_pool().await?;
        let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
            .fetch_one(&pool).await.unwrap_or(None);
        let last_date = last
            .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
            .unwrap_or(NaiveDate::from_ymd_opt(2021, 1, 1).unwrap());
        let floor = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let start_from = if last_date > floor { last_date - Duration::days(1) } else { last_date };
        let gap_days = (today - start_from).num_days().max(0) as u32;
        if gap_days > 7 {
            info!("sync inteligente: {} dias pendientes -> modo mensual (optimizado)", gap_days);
            let ranges = gen_month_range(start_from.year(), start_from.month(), today.year(), today.month(), today);
            let current_month = today.format("%Y-%m").to_string();
            for r in ranges {
                let csv = raw.join(format!("ventas_{}.csv", r.label));
                let incomplete = raw.join(format!("failed_{}.json", r.label)).exists();
                let exists = csv.exists()
                    && !incomplete
                    && std::fs::metadata(&csv).map(|m| m.len() > 1000).unwrap_or(false);
                // No saltar el mes actual: puede tener datos nuevos del dia
                if r.label == current_month || !exists {
                    pending.push(r);
                }
            }
        } else {
            info!("sync inteligente: {} dias pendientes -> modo diario (preciso)", gap_days);
            let mut d = start_from;
            while d <= today {
                let label = d.format("%Y-%m-%d").to_string();
                pending.push(MonthRange { start: d, end: d, label });
                d += Duration::days(1);
            }
        }
        info!("sync: last={} hoy={} -> {} items pendientes", last_date, today, pending.len());
    } else {
        let start_default = NaiveDate::from_ymd_opt(2021, 1, 1).unwrap();
        let s: NaiveDate = if _sd.is_empty() || _sd.contains("--") {
            start_default
        } else {
            NaiveDate::parse_from_str(&_sd.to_string(), "%Y-%m-%d").ok().unwrap_or(start_default)
        };
        let e: NaiveDate = if _ed.is_empty() || _ed.contains("--") {
            today
        } else {
            NaiveDate::parse_from_str(&_ed.to_string(), "%Y-%m-%d").ok().unwrap_or(today)
        };
        if s > e {
            info!("rango invalido (desde > hasta): {} -> {}", _sd, _ed);
            shared.lock().unwrap().set_phase(CapturePhase::Idle, "Rango invalido");
            return Ok(Vec::new());
        }
        let ranges = gen_month_range(s.year(), s.month(), e.year(), e.month(), e);
        let current_month = today.format("%Y-%m").to_string();
        for r in ranges {
            let csv = raw.join(format!("ventas_{}.csv", r.label));
            // No saltar el mes actual: aunque el CSV exista, puede tener datos nuevos
            // del dia (p.ej. hoy 31/08 con docs recientes). Se re-descarga completo.
            let incomplete = raw.join(format!("failed_{}.json", r.label)).exists();
            if r.label != current_month
                && !incomplete
                && csv.exists()
                && std::fs::metadata(&csv).map(|m| m.len() > 1000).unwrap_or(false)
            {
                continue;
            }
            pending.push(r);
        }
        info!("batch_history ({} -> {}): {} pendientes", _sd, _ed, pending.len());
    }
    Ok(pending)
}

/// Transporte: HTTP puro (primario) o Chrome (fallback si el login HTTP falla).
/// Cada worker usa su propia instancia (cookies separadas) para poder descargar en paralelo.
enum Transport {
    Http(crate::browser::http::CipsaHttp),
    Chrome(Box<crate::browser::captor::CipsaSession>),
}
impl Transport {
    async fn login(&self) -> Result<()> {
        match self {
            Transport::Http(s) => s.login().await,
            Transport::Chrome(s) => s.login().await,
        }
    }
    async fn download(&self, m: &MonthRange, raw: &std::path::Path) -> Result<std::path::PathBuf> {
        match self {
            Transport::Http(s) => s.download(m, raw).await,
            Transport::Chrome(s) => s.download(m, raw).await,
        }
    }
    async fn download_day_split(&self, d: &MonthRange, raw: &std::path::Path) -> Result<std::path::PathBuf> {
        match self {
            Transport::Http(s) => s.download_day_split(d, raw).await,
            Transport::Chrome(s) => s.download_day_split(d, raw).await,
        }
    }
    async fn scrape_html(&self, m: &MonthRange, raw: &std::path::Path) -> Result<std::path::PathBuf> {
        match self {
            Transport::Http(s) => s.scrape_html(m, raw).await,
            Transport::Chrome(s) => s.scrape_html(m, raw).await,
        }
    }
}

/// Crea un Transport (HTTP primario, Chrome fallback) con login incluido.
async fn create_transport() -> Result<Transport> {
    let http = crate::browser::http::CipsaHttp::new()?;
    match http.login().await {
        Ok(_) => Ok(Transport::Http(http)),
        Err(e) => {
            warn!("Login HTTP fallo ({e:#}) — fallback a Chrome");
            let cs = crate::browser::captor::CipsaSession::new()
                .map_err(|e| anyhow!("No se pudo iniciar Chrome ni HTTP: {e:?}"))?;
            cs.login().await.map_err(|e2| anyhow!("Login fallido (HTTP y Chrome): {e} / {e2}"))?;
            Ok(Transport::Chrome(Box::new(cs)))
        }
    }
}

/// Procesa un solo mes: descarga (con reintentos + granular + dia-a-dia + HTML fallback)
/// y parsea+inserta. Retorna Ok(true) si fallo sistémico del servidor, Ok(false) si ok/parcial.
async fn process_month(
    transport: &mut Transport,
    m: &MonthRange,
    raw: &std::path::Path,
    shared: &SharedProgress,
    idx: usize,
    total: usize,
    start_instant: &Instant,
    abort_flag: Option<&Arc<AtomicBool>>,
    ok: &mut Vec<String>,
    fail: &mut Vec<(String, String)>,
) -> Result<bool> {
    let pct = (idx as f32 / total as f32) * 0.85;
    let t_item = Instant::now();
    let aborted = abort_flag.map(|f| f.load(Ordering::Relaxed)).unwrap_or(false)
        || CAPTURE_ABORT_GLOBAL.load(Ordering::Relaxed);
    if aborted {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::Error, "Captura abortada por el usuario");
        s.update_progress(pct, "Abortada");
        return Err(anyhow!("Abortado por usuario"));
    }
    {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::Downloading, format!("Descargando {}...", m.label));
        s.set_current(&m.label);
        s.update_progress(pct, format!("[{}/{}] Descargando {}", idx + 1, total, m.label));
    }
    info!("=== {}/{}: {} ===", idx + 1, total, m.label);
    let is_enero = m.label.ends_with("-01");
    let mut success = false;
    let mut systemic = false;
    if !is_enero {
        for attempt in 1..=3 {
            {
                let mut s = shared.lock().unwrap();
                s.update_progress(pct, format!("Intento {}/3 — {}", attempt, m.label));
                s.set_current(&format!("{} (intento {})", m.label, attempt));
            }
            info!("  intento {}/3", attempt);
            let res = transport.download(m, raw).await;
            match res {
                Ok(xls) => {
                    info!("  Descargado: {}", xls.display());
                    let csv_path = raw.join(format!("ventas_{}.csv", m.label));
                    // El .xls recien descargado SIEMPRE se reconvierte (aunque exista un CSV
                    // viejo): el mes actual se re-descarga a diario y el CSV previo quedaria
                    // desactualizado (bug: Supabase quedaba en una fecha anterior).
                    if xls.exists() {
                        if let Err(e) = crate::processor::xls::xls_to_csv(&xls, &csv_path) {
                            let msg = format!("Error convirtiendo {}: {e:#}", m.label);
                            warn!("  {msg}");
                            let mut s = shared.lock().unwrap();
                            s.update_progress(pct, &msg);
                            continue;
                        }
                    }
                    {
                        let mut s = shared.lock().unwrap();
                        s.set_phase(CapturePhase::Parsing, format!("Parseando {}", m.label));
                        s.set_current(&m.label);
                        s.update_progress(pct + 0.02, format!("[{}/{}] Parseando {}", idx + 1, total, m.label));
                    }
                    if csv_path.exists() {
                        let pool = init_pool().await?;
                        if let Ok(ventas) = parse_export_csv_with_cross(&csv_path, &pool).await {
                            let n = insert_with_retry(&pool, &ventas, &m.label).await?;
                            info!("  Normalizado: {} filas -> {}", m.label, n);
                            {
                                let mut s = shared.lock().unwrap();
                                s.set_phase(CapturePhase::Parsing, format!("Insertadas {} filas", n));
                                s.update_progress(pct + 0.03, format!("[{}/{}] {} filas insertadas", idx + 1, total, n));
                            }
                        }
                    }
                    let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label)));
                    success = true;
                    break;
                }
                Err(e) => {
                    warn!("  fail {}: {}", attempt, e);
                    let es = format!("{e}");
                    if es.contains("Timeout") || es.contains("timed out") || es.contains("deadline") {
                        warn!("  timeout detectado — probe rapido de 1 dia para confirmar");
                        let probe_label = format!("{}-probe", m.start.format("%Y-%m-%d"));
                        let probe = MonthRange { start: m.start, end: m.start, label: probe_label };
                        let probe_res = tokio::time::timeout(
                            StdDuration::from_secs(120),
                            transport.download(&probe, raw),
                        ).await;
                        let _ = std::fs::remove_file(raw.join(format!("ventas_{}.xls", probe.label)));
                        let _ = std::fs::remove_file(raw.join(format!("ventas_{}.csv", probe.label)));
                        let server_dead = matches!(probe_res, Ok(Err(_))) || probe_res.is_err();
                        if server_dead {
                            systemic = true;
                            break;
                        }
                        info!("  probe OK — servidor volvio, reintentando mes");
                    } else {
                        sleep(StdDuration::from_secs(3)).await;
                    }
                }
            }
        }
    }

    if systemic {
        return Ok(true);
    }

    if !success {
        let mut recovered = false;
        {
            let mut dias_ok = 0usize;
            let mut dias_tot = 0usize;
            let mut cur0 = m.start;
            while cur0 <= m.end {
                dias_tot += 1;
                let f = raw.join(format!("ventas_{}.csv", cur0.format("%Y-%m-%d")));
                if f.exists() && std::fs::metadata(&f).map(|mm| mm.len()>1000).unwrap_or(false) { dias_ok += 1; }
                cur0 = cur0 + Duration::days(1);
            }
            if dias_tot > 0 && dias_ok == dias_tot {
                info!("  {} todos los dailies presentes ({}/{}), merge directo", m.label, dias_ok, dias_tot);
                let merged = raw.join(format!("ventas_{}.csv", m.label));
                let mut parts0 = Vec::new();
                let mut cur1 = m.start;
                while cur1 <= m.end {
                    let f = raw.join(format!("ventas_{}.csv", cur1.format("%Y-%m-%d")));
                    if f.exists() { parts0.push(f); }
                    cur1 = cur1 + Duration::days(1);
                }
                if merge_csvs(&parts0, &merged).is_ok() && merged.exists() && std::fs::metadata(&merged).map(|mm| mm.len()>1000).unwrap_or(false) {
                    let pool = init_pool().await?;
                    match parse_export_csv_with_cross(&merged, &pool).await {
                        Ok(ventas) => match insert_with_retry(&pool, &ventas, &m.label).await {
                            Ok(n) => {
                                info!("  merge directo {} filas -> {}", m.label, n);
                                recovered = true;
                                ok.push(m.label.clone());
                                let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label)));
                            }
                            Err(e) => warn!("  merge directo insert fail: {}", e),
                        },
                        Err(e) => warn!("  merge directo parse fail: {}", e),
                    }
                }
            }
        }
        if !recovered {
            let parts_list: Vec<usize> = if is_enero { vec![4,6,10] } else { vec![2,4,6,10] };
            for parts in parts_list {
                warn!("  {} fallo, probando {} partes", m.label, parts);
                let chunks = split_month_n(m, parts);
                let mut ok_chunks = 0;
                for h in &chunks {
                    let mut h_ok = false;
                    for ha in 1..=2 {
                        let res_h = transport.download(h, raw).await;
                        match res_h {
                            Ok(xls_h) => {
                                let csv_h = raw.join(format!("ventas_{}.csv", h.label));
                                match crate::processor::xls::xls_to_csv(&xls_h, &csv_h) {
                                    Ok(_) if csv_h.exists() && std::fs::metadata(&csv_h).map(|m| m.len() > 500).unwrap_or(false) => {
                                        h_ok = true;
                                        break;
                                    }
                                    Ok(_) => warn!("  chunk {} xls_to_csv sin csv valido", h.label),
                                    Err(e) => warn!("  chunk {} xls_to_csv failed: {}", h.label, e),
                                }
                            }
                            Err(e) => {
                                warn!("  chunk {} intento {} fail: {}", h.label, ha, e);
                                sleep(StdDuration::from_secs(2)).await;
                            }
                        }
                    }
                    if h_ok { ok_chunks += 1; } else { break; }
                    sleep(StdDuration::from_secs(1)).await;
                }
                if ok_chunks == chunks.len() {
                    let merged = raw.join(format!("ventas_{}.csv", m.label));
                    let mut parts = Vec::new();
                    for h in &chunks {
                        let part_csv = raw.join(format!("ventas_{}.csv", h.label));
                        if part_csv.exists() { parts.push(part_csv); }
                    }
                    match merge_csvs(&parts, &merged) {
                        Ok(n) => info!("  granular merge rust: {} filas -> {}", n, merged.display()),
                        Err(e) => warn!("  granular merge rust failed: {e:#}"),
                    }
                    let merged_exists = merged.exists() && std::fs::metadata(&merged).map(|m| m.len() > 1000).unwrap_or(false);
                    if merged_exists {
                        match parse_export_csv_with_cross(&merged, &init_pool().await?).await {
                            Ok(ventas) if !ventas.is_empty() => {
                                match init_pool().await {
                                    Ok(pool) => {
                                        match insert_with_retry(&pool, &ventas, &m.label).await {
                                            Ok(n) => {
                                                info!("  granular insert {} filas -> {}", m.label, n);
                                                recovered = true;
                                                info!("  granular {} partes OK {}", parts.len(), m.label);
                                            },
                                            Err(e) => warn!("  granular insert fail {}: {}", m.label, e),
                                        }
                                    }
                                    Err(e) => warn!("  pool fail: {}", e),
                                }
                            }
                            Ok(_) => warn!("  granular parse vacio {} (0 filas)", m.label),
                            Err(e) => warn!("  granular parse fail {}: {}", m.label, e),
                        }
                    } else {
                        warn!("  granular {} merge no produjo CSV valido ({} bytes)", parts.len(), std::fs::metadata(&merged).map(|m| m.len()).unwrap_or(0));
                    }
                    if recovered {
                        let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label)));
                        break;
                    } else { continue; }
                }
            }
        }
        if !recovered {
            warn!("  {} granular 10 fallo, probando dia-a-dia", m.label);
            let mut cur = m.start;
            let mut day_ok = 0;
            let mut day_total = 0;
            let mut consec_fail = 0usize;
            while cur <= m.end {
                day_total += 1;
                let label = cur.format("%Y-%m-%d").to_string();
                let day_range = MonthRange { start: cur, end: cur, label: label.clone() };
                let day_csv = raw.join(format!("ventas_{}.csv", label));
                let day_xls = raw.join(format!("ventas_{}.xls", label));
                let mut d_ok = false;
                if day_csv.exists() && std::fs::metadata(&day_csv).map(|m| m.len() > 1000).unwrap_or(false) {
                    d_ok = true;
                } else if day_xls.exists() && std::fs::metadata(&day_xls).map(|m| m.len() > 1000).unwrap_or(false) {
                    if crate::processor::xls::xls_to_csv(&day_xls, &day_csv).is_ok() && day_csv.exists() && std::fs::metadata(&day_csv).map(|m| m.len() > 500).unwrap_or(false) {
                        d_ok = true;
                    }
                } else {
                    for _ha in 1..=2 {
                        if _ha == 2 { let _ = transport.login().await; }
                        let res_d = transport.download(&day_range, raw).await;
                        match res_d {
                            Ok(xls_d) => {
                                let csv_d = raw.join(format!("ventas_{}.csv", label));
                                if crate::processor::xls::xls_to_csv(&xls_d, &csv_d).is_ok() && csv_d.exists() && std::fs::metadata(&csv_d).map(|m| m.len() > 500).unwrap_or(false) {
                                    d_ok = true;
                                    break;
                                } else { warn!("  day {} xls_to_csv fallo", label); }
                            }
                            Err(_) => {}
                        }
                        sleep(StdDuration::from_secs(2)).await;
                    }
                    if !d_ok {
                        warn!("  dia {} fail normal, probando split Art", label);
                        match transport.download_day_split(&day_range, raw).await {
                            Ok(_) => {
                                if day_csv.exists() && std::fs::metadata(&day_csv).map(|m| m.len()>1000).unwrap_or(false) {
                                    d_ok = true;
                                    info!("  dia {} split Art OK", label);
                                }
                            }
                            Err(e) => warn!("  dia {} split Art fail: {}", label, e),
                        }
                    }
                }
                if d_ok {
                    day_ok += 1;
                    consec_fail = 0;
                } else {
                    consec_fail += 1;
                    if consec_fail == 3 {
                        warn!("  3 dias seguidos fallan — backoff 15s + re-login");
                        sleep(StdDuration::from_secs(15)).await;
                        let _ = transport.login().await;
                    } else if consec_fail >= 6 {
                        warn!("  {} dias seguidos sin exito — servidor caido", consec_fail);
                        {
                            let mut s = shared.lock().unwrap();
                            s.update_progress(pct, format!("{}: servidor no responde tras {} días", m.label, consec_fail));
                        }
                        break;
                    }
                }
                cur = cur + Duration::days(1);
                sleep(StdDuration::from_secs(1)).await;
            }
            if day_ok == day_total && day_total > 0 {
                let merged = raw.join(format!("ventas_{}.csv", m.label));
                let mut parts2 = Vec::new();
                let mut cur2 = m.start;
                while cur2 <= m.end {
                    let f = raw.join(format!("ventas_{}.csv", cur2.format("%Y-%m-%d")));
                    if f.exists() { parts2.push(f); }
                    cur2 = cur2 + Duration::days(1);
                }
                match merge_csvs(&parts2, &merged) {
                    Ok(n) => info!("  day-a-day merge rust: {} filas -> {}", n, merged.display()),
                    Err(e) => warn!("  day-a-day merge rust failed: {e:#}"),
                }
                if merged.exists() {
                    let pool = init_pool().await?;
                    match parse_export_csv_with_cross(&merged, &pool).await {
                        Ok(ventas) => {
                            match insert_with_retry(&pool, &ventas, &m.label).await {
                                Ok(n) => info!("  dia-a-dia insert {} filas -> {}", m.label, n),
                                Err(e) => warn!("  dia-a-dia insert fail {}: {}", m.label, e),
                            }
                        }
                        Err(e) => warn!("  dia-a-dia parse fail {}: {}", m.label, e),
                    }
                    let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label)));
                    recovered = true;
                    info!("  dia-a-dia OK {} ({}/{})", m.label, day_ok, day_total);
                }
            } else {
                let mut faltantes: Vec<String> = Vec::new();
                let mut cur_missing = m.start;
                while cur_missing <= m.end {
                    let f = raw.join(format!("ventas_{}.csv", cur_missing.format("%Y-%m-%d")));
                    if !(f.exists() && std::fs::metadata(&f).map(|mm| mm.len() > 1000).unwrap_or(false)) {
                        faltantes.push(cur_missing.format("%Y-%m-%d").to_string());
                    }
                    cur_missing += Duration::days(1);
                }
                warn!("  {} incompleto ({}/{} dias); no se inserta", m.label, day_ok, day_total);
                let fail_path = raw.join(format!("failed_{}.json", m.label));
                let _ = std::fs::write(&fail_path, serde_json::to_string(&faltantes).unwrap_or_default());
                let _ = std::fs::remove_file(raw.join(format!("ventas_{}.csv", m.label)));
            }
            if !recovered {
                warn!("  {} dia-a-dia incompleto, probando fallback HTML L0", m.label);
                match transport.scrape_html(m, raw).await {
                    Ok(_xls) => {
                        let csv = raw.join(format!("ventas_{}.csv", m.label));
                        if csv.exists() {
                            let pool = init_pool().await?;
                            match parse_export_csv_with_cross(&csv, &pool).await {
                                Ok(ventas) if !ventas.is_empty() => {
                                    match insert_with_retry(&pool, &ventas, &m.label).await {
                                        Ok(n) => { info!("  HTML fallback insert {} filas -> {}", m.label, n); recovered = true; let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label))); },
                                        Err(e) => warn!("  HTML insert fail: {}", e),
                                    }
                                }
                                Ok(_) => warn!("  HTML parse vacio"),
                                Err(e) => warn!("  HTML parse fail: {}", e),
                            }
                        }
                    }
                    Err(e) => warn!("  HTML fallback fail {}: {}", m.label, e),
                }
            }
        }
        if recovered {
            ok.push(m.label.clone());
        } else {
            let mut faltantes: Vec<String> = Vec::new();
            let mut cur3 = m.start;
            while cur3 <= m.end {
                let f = raw.join(format!("ventas_{}.csv", cur3.format("%Y-%m-%d")));
                if !(f.exists() && std::fs::metadata(&f).map(|mm| mm.len()>1000).unwrap_or(false)) {
                    faltantes.push(cur3.format("%Y-%m-%d").to_string());
                }
                cur3 = cur3 + Duration::days(1);
            }
            if !faltantes.is_empty() {
                let fail_path = raw.join(format!("failed_{}.json", m.label));
                let _ = std::fs::write(&fail_path, serde_json::to_string(&faltantes).unwrap_or_default());
                warn!("  {} fail total — dias faltantes: {:?}", m.label, faltantes);
            }
            let detail = if faltantes.is_empty() { "3 intentos + granular 2/4/6/10/dia fallo".to_string() } else { format!("faltan dias {} — sube XLS manual", faltantes.join(",")) };
            fail.push((m.label.clone(), detail));
        }
    } else {
        ok.push(m.label.clone());
    }

    let done_count = ok.len() + fail.len();
    if done_count > 0 {
        let elapsed = start_instant.elapsed().as_secs();
        let avg = elapsed as f64 / done_count as f64;
        let remaining = (total.saturating_sub(done_count)) as f64;
        let eta = (avg * remaining).ceil() as u64;
        let mut s = shared.lock().unwrap();
        s.eta_secs = Some(eta);
        let eta_m = eta / 60;
        let eta_s = eta % 60;
        let base_msg = s.message.clone();
        let prefix = base_msg.split(" — ").next().unwrap_or(&base_msg);
        s.update_progress(pct, format!("[{}/{}] {} — quedan ~{}m{}s", done_count, total, prefix, eta_m, eta_s));
    }

    // Pacing: cada worker mantiene su propio pacing (server rate-limit)
    if idx + 1 < total {
        let item_secs = t_item.elapsed().as_secs();
        let extra = if item_secs > 60 { (item_secs / 8).min(30) } else { 0 };
        let wait = SLEEP_BETWEEN_MONTHS + extra;
        if extra > 0 {
            info!("  pacing: mes tardo {item_secs}s -> pausa extendida {wait}s (base {} + extra {extra})", SLEEP_BETWEEN_MONTHS);
        }
        sleep(StdDuration::from_secs(wait)).await;
    }

    Ok(systemic)
}

pub async fn run_batch_history(
    _sd: &str,
    _ed: &str,
    daily: bool,
    shared: SharedProgress,
    abort_flag: Option<Arc<AtomicBool>>,
    workers: usize,
) -> Result<()> {
    let raw = raw_dir();
    std::fs::create_dir_all(&raw)?;

    // Prechequeos: credenciales y Chrome, falla rapido si falta algo
    if let Err(pmsg) = crate::browser::captor::preflight_checks() {
        {
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Error, &pmsg);
            s.update_progress(0.0, &pmsg);
        }
        return Err(anyhow!(pmsg));
    }

    // Verificacion de lock exclusivo
    {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::CheckingLock, "Verificando lock...");
        s.update_progress(0.0, "Verificando lock...");
    }

    let _lock_guard = match acquire_lock(&raw) {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("{e}");
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Error, &msg);
            s.update_progress(0.0, &msg);
            return Err(anyhow!(msg));
        }
    };
    info!("Lock adquirido: {}", _lock_guard.path.display());

    // Parse user date range (non-daily mode)
    let (user_start, user_end) = if daily {
        (None, None)
    } else {
        let s_d: NaiveDate = if _sd.is_empty() || _sd.contains("--") {
            NaiveDate::from_ymd_opt(2021, 1, 1).unwrap()
        } else {
            NaiveDate::parse_from_str(_sd, "%Y-%m-%d").ok().unwrap_or(NaiveDate::from_ymd_opt(2021, 1, 1).unwrap())
        };
        let e_d: NaiveDate = if _ed.is_empty() || _ed.contains("--") {
            chrono::Local::now().naive_local().date()
        } else {
            NaiveDate::parse_from_str(_ed, "%Y-%m-%d").ok().unwrap_or(chrono::Local::now().naive_local().date())
        };
        if s_d > e_d {
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Idle, "Rango invalido");
            return Ok(());
        }
        (Some(s_d), Some(e_d))
    };

    let start_instant = Instant::now();
    let total_estimated = if daily {
        let pool = init_pool().await?;
        let last: Option<String> = sqlx::query_scalar("SELECT MAX(fecha_orig) FROM ventas")
            .fetch_one(&pool)
            .await
            .unwrap_or(None);
        let last_date = last
            .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
            .unwrap_or(NaiveDate::from_ymd_opt(2021, 1, 1).unwrap());
        let today = chrono::Local::now().naive_local().date();
        (today - last_date).num_days().max(0) as usize + 1
    } else {
        let s_d = user_start.unwrap();
        let e_d = user_end.unwrap();
        (e_d - s_d).num_days() as usize / 31 + 1
    };

    {
        let mut s = shared.lock().unwrap();
        s.set_start(&format!("Captura iniciada — {total_estimated} item(s) estimado(s)"));
    }

    let progress_handle = spawn_progress_writer(shared.clone()).await;

    let pending = build_pending(_sd, _ed, daily, &raw, shared.clone()).await?;

    if pending.is_empty() {
        {
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Done, "No hay datos pendientes");
            s.update_progress(1.0, "No hay datos pendientes");
        }
        info!("Nada pendiente");
        progress_handle.abort();
        return Ok(()); // _lock_guard se libera automaticamente
    }

    let total = pending.len();
    info!("Total items a procesar: {}", total);

    let mut ok = Vec::new();
    let mut fail = Vec::new();
    let mut sys_fail = 0usize; // meses consecutivos sin respuesta del servidor

    // Reset flag global de abort al iniciar batch (abort_capture lo pone en true)
    CAPTURE_ABORT_GLOBAL.store(false, Ordering::Relaxed);

    let num_workers = if workers == 0 { 1 } else { workers };
    if num_workers == 1 {
        // Ruta secuencial (comportamiento original)
        let mut session = create_transport().await?;
        for (idx, m) in pending.iter().enumerate() {
            let pct = (idx as f32 / total as f32) * 0.85;
            let aborted = abort_flag.as_ref().map(|f| f.load(Ordering::Relaxed)).unwrap_or(false)
                || CAPTURE_ABORT_GLOBAL.load(Ordering::Relaxed);
            if aborted {
                let mut s = shared.lock().unwrap();
                s.set_phase(CapturePhase::Error, "Captura abortada por el usuario");
                s.update_progress(pct, "Abortada");
                progress_handle.abort();
                return Err(anyhow!("Abortado por usuario"));
            }
            if idx > 0 && idx % 6 == 0 {
                info!("  refresh sesion (mes {} de {})", idx + 1, total);
                let _ = session.login().await;
            }
            // Cuerpo unico del mes: process_month. Antes existia aqui una copia
            // inline de ~390 lineas (ruta secuencial) divergente en refresh de
            // sesion/sys_fail/comentarios; unificada en el paso 4 de
            // docs/REFACTOR_PLAN.md. process_month hace: descarga con reintentos,
            // fallbacks (merge diario / granular / por dia / HTML), parse+insert,
            // ETA y pacing inter-mes.
            let sys = process_month(
                &mut session, m, &raw, &shared, idx, total, &start_instant,
                abort_flag.as_ref(), &mut ok, &mut fail,
            ).await?;
            if sys {
                // process_month retorna systemic sin registrar en `fail` (igual
                // que la ruta concurrente). Aqui se conserva el registro que
                // tenia la ruta secuencial.
                sys_fail += 1;
                fail.push((
                    m.label.clone(),
                    "servidor intranet sin respuesta (timeout)".to_string(),
                ));
                warn!("  {} SYSTEMIC ({}/3)", m.label, sys_fail);
                {
                    let mut s = shared.lock().unwrap();
                    s.update_progress(
                        pct,
                        format!("{}: servidor sin respuesta ({}/{})", m.label, sys_fail, 3),
                    );
                }
                if sys_fail >= 3 {
                    let msg = "Servidor intranet caido (3 meses seguidos sin respuesta). Reintente en 30-60 min o en horario laboral.".to_string();
                    let mut s = shared.lock().unwrap();
                    s.set_phase(CapturePhase::Error, &msg);
                    s.update_progress(pct, &msg);
                    drop(s);
                    warn!("{}", msg);
                    progress_handle.abort();
                    return Err(anyhow!(msg));
                }
                continue;
            }
            sys_fail = 0;
        }
    } else {
        // Ruta concurrente: N workers con sesiones independientes
        info!("Iniciando captura concurrente con {} workers", num_workers);
        // Clonar meses en blobs independientes para que los spawns tengan 'static
        let chunk_owned: Vec<Vec<MonthRange>> = pending.chunks((total + num_workers - 1) / num_workers)
            .map(|c| c.to_vec())
            .collect();
        let n_chunks = chunk_owned.len();
        let mut handles: Vec<tokio::task::JoinHandle<Result<(Vec<String>, Vec<(String, String)>)>>> = Vec::with_capacity(num_workers);
        for (wi, chunk) in chunk_owned.iter().enumerate() {
            if chunk.is_empty() { continue; }
            let shared_w = shared.clone();
            let abort_w = abort_flag.clone();
            let raw_w = raw.clone();
            let total_w = total;
            let n_c = n_chunks;
            let chunk_clone = chunk.clone();
            let handle = tokio::spawn(async move {
                let mut transport = match create_transport().await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!("Worker {} login falló: {e}", wi);
                        return Ok((Vec::new(), Vec::new()));
                    }
                };
                let mut w_ok = Vec::new();
                let mut w_fail = Vec::new();
                let mut w_sys = 0usize;
                let w_start = Instant::now();
                for (li, m) in chunk_clone.iter().enumerate() {
                    let g_idx = wi * n_c + li;
                    let sys = match process_month(
                        &mut transport, m, &raw_w, &shared_w, g_idx, total_w, &w_start, abort_w.as_ref(),
                        &mut w_ok, &mut w_fail,
                    ).await {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("Worker {} mes {} error: {e}", wi, m.label);
                            w_fail.push((m.label.clone(), format!("error: {e}")));
                            continue;
                        }
                    };
                    if sys {
                        w_sys += 1;
                        if w_sys >= 3 {
                            let msg = "Servidor intranet caido (3 meses seguidos sin respuesta). Reintente en 30-60 min.".to_string();
                            let mut s = shared_w.lock().unwrap();
                            s.set_phase(CapturePhase::Error, &msg);
                            s.update_progress(0.0, &msg);
                            return Ok((w_ok, w_fail));
                        }
                    }
                    // Refresh sesion cada 6 meses por worker
                    if li > 0 && li % 6 == 0 {
                        info!("  worker {} refresh sesion (mes {} de {})", wi, li + 1, chunk_clone.len());
                        let _ = transport.login().await;
                    }
                }
                Ok((w_ok, w_fail))
            });
            handles.push(handle);
        }
        // Await all workers
        for handle in handles {
            match handle.await {
                Ok(Ok((wk, wf))) => {
                    ok.extend(wk);
                    fail.extend(wf);
                }
                Ok(Err(e)) => warn!("Worker task error: {e}"),
                Err(e) => warn!("Worker join error: {e}"),
            }
        }
    }


    let elapsed = start_instant.elapsed();
    let elapsed_min = elapsed.as_secs() / 60;
    let elapsed_sec = elapsed.as_secs() % 60;

    let pool = init_pool().await?;
    // Stats cache al final (evita 4 querys pesados con filtro por mes durante captura)
    {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::Normalizing, "Actualizando estadisticas...");
        s.set_current("stats");
        s.update_progress(0.95, "Actualizando estadisticas...");
    }
    let _ = crate::db::writer::refresh_stats_cache(&pool).await;
    // Estadisticas del planner al dia tras la carga (mejora los planes de consulta).
    let _ = crate::db::writer::analyze_db(&pool).await;

    // Finalizacion — incluir dias faltantes en el mensaje para que UI no solo diga "fail"
    {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::Done, format!("Completado en {:02}:{:02}", elapsed_min, elapsed_sec));
        let base = format!("Listo — {}/{} ok, {}/{} fail", ok.len(), total, fail.len(), total);
        let detail = if fail.is_empty() { String::new() } else {
            let mut d = String::new();
            for (mes, det) in fail.iter().take(2) {
                if !d.is_empty() { d.push_str("; "); }
                d.push_str(&format!("{}: {}", mes, det));
            }
            if fail.len()>2 { d.push_str(" ..."); }
            d
        };
        let msg = if detail.is_empty() { base } else { format!("{} — {}", base, detail) };
        s.update_progress(1.0, msg);
        s.set_current("");
    }

    info!("DONE {}/{} ok, {}/{} fail (elapsed {:02}:{:02})",
        ok.len(), total, fail.len(), total, elapsed_min, elapsed_sec);

    // Reporte de alertas estructuradas
    {
        let mut alerts = crate::alerts::AlertCollector::new();
        for (mes, motivo) in &fail {
            if motivo.contains("locked") {
                alerts.auto_fixable(mes, motivo);
            } else if motivo.contains("timeout") || motivo.contains("servidor") {
                alerts.critical(mes, motivo, "Verificar intranet y reintentar con recapture");
            } else {
                alerts.critical(mes, motivo, "Revisar CSV manualmente y re-importar con importxls");
            }
        }
        alerts.report();
    }

    progress_handle.abort();
    Ok(()) // _lock_guard se libera automaticamente
}

fn gen_month_range(sy: i32, sm: u32, ey: i32, em: u32, e_d: NaiveDate) -> Vec<MonthRange> {
    let mut out = Vec::new();
    let (mut y, mut m) = (sy, sm);
    loop {
        let s = NaiveDate::from_ymd_opt(y, m, 1).unwrap();
        let (ey2, em2) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        let mut e = NaiveDate::from_ymd_opt(ey2, em2, 1).unwrap() - Duration::days(1);
        // Clamp the last month to the user's actual end date
        if y == ey && m == em && e > e_d {
            e = e_d;
        }
        out.push(MonthRange {
            start: s,
            end: e,
            label: format!("{}-{:02}", y, m),
        });
        if m == 12 {
            if y == ey { break; }
            y += 1; m = 1;
        } else if y == ey && m == em {
            break;
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
