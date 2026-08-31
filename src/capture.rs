use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, Duration, NaiveDate};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::browser::captor::{CipsaSession, MonthRange};
use crate::config::{raw_dir, SLEEP_BETWEEN_MONTHS};
use crate::db::writer::{dedup_ventas, init_pool, insert_ventas};
use crate::processor::parser::{parse_export_csv, parse_export_csv_with_cross};
use crate::capture_state::{CapturePhase, ProgressState, SharedProgress, now_secs};

use std::sync::atomic::{AtomicBool, Ordering};

const LOCK_FILE: &str = "capture.lock";

/// Flag global de abort — compartido con http.rs para cortar GET/POST largos.
pub static CAPTURE_ABORT_GLOBAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Merge de CSVs en Rust puro: header del primero una vez, resto data.
pub fn merge_csvs(parts: &[PathBuf], merged: &PathBuf) -> Result<usize> {
    use std::io::{BufRead, BufWriter, Write};
    let mut out = BufWriter::new(std::fs::File::create(merged)?);
    let mut total = 0usize;
    let mut wrote_header = false;
    for p in parts {
        let f = std::fs::File::open(p).with_context(|| format!("abrir {}", p.display()))?;
        let reader = std::io::BufReader::new(f);
        for (j, line) in reader.lines().enumerate() {
            let line = line?;
            // Header (primera linea): se escribe una sola vez y no cuenta como data
            if j == 0 && !wrote_header && !line.trim().is_empty() {
                out.write_all(line.as_bytes())?;
                out.write_all(b"\n")?;
                wrote_header = true;
                continue;
            }
            // Headers de archivos siguientes se saltan
            if j == 0 {
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
        drop(&self._file);
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
        loop {
            sleep(StdDuration::from_secs(2)).await;
            let s = shared.lock().unwrap();
            let elapsed = if let Some(start) = s.started_at {
                now_secs().saturating_sub(start)
            } else {
                0
            };
            let elapsed_min = elapsed / 60;
            let elapsed_sec = elapsed % 60;
            info!(
                "[PROGRESS] phase={} progress={:.1}% msg=\"{}\" current=\"{}\" elapsed={:02}:{:02}",
                s.phase, s.progress * 100.0, s.message, s.current_item, elapsed_min, elapsed_sec
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
                let exists = csv.exists() && std::fs::metadata(&csv).map(|m| m.len() > 1000).unwrap_or(false);
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
            if r.label != current_month
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

pub async fn run_batch_history(
    _sd: &str,
    _ed: &str,
    daily: bool,
    shared: SharedProgress,
    abort_flag: Option<Arc<AtomicBool>>,
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
        ((e_d - s_d).num_days() as usize / 31 + 1)
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

    // Transporte: HTTP puro primario (~2-3x mas rapido, sin timeouts de recursos del browser);
    // Chrome queda como fallback solo si el login HTTP falla (cambio de flujo server-side)
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

    let mut session: Transport = {
        let http = crate::browser::http::CipsaHttp::new();
        match http {
            Ok(h) => match h.login().await {
                Ok(_) => Transport::Http(h),
                Err(e) => {
                    warn!("Login HTTP fallo ({e:#}) — fallback a Chrome");
                    let cs = crate::browser::captor::CipsaSession::new()
                        .map_err(|e| anyhow!("No se pudo iniciar Chrome ni HTTP: {e:?}"))?;
                    if let Err(e2) = cs.login().await {
                        let msg = format!("Login fallido (HTTP y Chrome): {e} / {e2}");
                        let mut st = shared.lock().unwrap();
                        st.set_phase(CapturePhase::Error, &msg);
                        st.update_progress(0.0, &msg);
                        return Err(anyhow!(msg));
                    }
                    Transport::Chrome(Box::new(cs))
                }
            },
            Err(e) => {
                let msg = format!("No se pudo crear cliente HTTP: {e}");
                let mut st = shared.lock().unwrap();
                st.set_phase(CapturePhase::Error, &msg);
                st.update_progress(0.0, &msg);
                return Err(anyhow!(msg));
            }
        }
    };

    // Reset flag global de abort al iniciar batch (abort_capture lo pone en true)
    CAPTURE_ABORT_GLOBAL.store(false, Ordering::Relaxed);

    for (idx, m) in pending.iter().enumerate() {
        let pct = (idx as f32 / total as f32) * 0.85;
        let t_item = Instant::now(); // duracion del mes -> pacing adaptativo al final
        // Check abort flag (local + global: global lo setea abort_capture desde otro proceso/ventana)
        let aborted = abort_flag.as_ref().map(|f| f.load(Ordering::Relaxed)).unwrap_or(false)
            || CAPTURE_ABORT_GLOBAL.load(Ordering::Relaxed);
        if aborted {
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Error, "Captura abortada por el usuario");
            s.update_progress(pct, "Abortada");
            progress_handle.abort();
            return Err(anyhow!("Abortado por usuario"));
        }
        // Refresh de sesion cada 6 meses (cookie expira a mitad de batch -> meses enteros fallan)
        if idx > 0 && idx % 6 == 0 {
            info!("  refresh sesion (mes {} de {})", idx + 1, total);
            let _ = session.login().await;
        }
        {
            let mut s = shared.lock().unwrap();
            s.set_phase(CapturePhase::Downloading, format!("Descargando {}...", m.label));
            s.set_current(&m.label);
            s.update_progress(pct, format!("[{}/{}] Descargando {}", idx + 1, total, m.label));
        }
        info!("=== {}/{}: {} ===", idx + 1, total, m.label);
        // Enero siempre grande (2024-01 31k filas/40MB): ir granular desde el inicio.
        // El GET directo de un mes completo de enero excede 420s de render -> no perder 3 intentos.
        let is_enero = m.label.ends_with("-01");
        let mut success = false;
        let mut systemic = false; // timeout del servidor (no es error de datos)
        if !is_enero {
            for attempt in 1..=3 {
            {
                let mut s = shared.lock().unwrap();
                s.update_progress(pct, format!("Intento {}/3 — {}", attempt, m.label));
                s.set_current(&format!("{} (intento {attempt})", m.label));
            }
            info!("  intento {}/3", attempt);
            let res = session.download(m, &raw).await;
            match res {
                Ok(xls) => {
                    info!("  Descargado: {}", xls.display());
                    // Convertir XLS -> CSV con errores VISIBLES
                    let csv_path = raw.join(format!("ventas_{}.csv", m.label));
                    if xls.exists() && !csv_path.exists() {
                        if let Err(e) = crate::processor::xls::xls_to_csv(&xls, &csv_path) {
                            let msg = format!("Error convirtiendo {}: {e:#}", m.label);
                            warn!("  {msg}");
                            let mut s = shared.lock().unwrap();
                            s.update_progress(pct, &msg);
                            continue; // reintento (siguiente intento del loop)
                        }
                    }

                    // Parse and insert into DB
                    {
                        let mut s = shared.lock().unwrap();
                        s.set_phase(CapturePhase::Parsing, format!("Parseando {}", m.label));
                        s.set_current(&m.label);
                        s.update_progress(pct + 0.02, format!("[{}/{}] Parseando {}", idx + 1, total, m.label));
                    }

                    if csv_path.exists() {
                            let pool = init_pool().await?;
                            if let Ok(ventas) = parse_export_csv_with_cross(&csv_path, &pool).await {
                                let n = insert_ventas(&pool, &ventas).await?;
                                info!("  Normalizado: {} filas -> {}", m.label, n);
                                {
                                    let mut s = shared.lock().unwrap();
                                    s.set_phase(CapturePhase::Parsing, format!("Insertadas {} filas", n));
                                    s.update_progress(pct + 0.03, format!("[{}/{}] {} filas insertadas", idx + 1, total, n));
                                }
                            }
                        }
                    // Limpiar stale failed marker si quedo de un intento anterior
                    let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label)));
                    success = true;
                    sys_fail = 0;
                    break;
                }
                Err(e) => {
                    warn!("  fail {}: {}", attempt, e);
                    let es = format!("{e}");
                    if es.contains("Timeout") || es.contains("timed out") || es.contains("deadline") {
                        // Probe barato: 1 dia. Si tambien cuelga, el servidor esta caido — no quemar cascada granular.
                        warn!("  timeout detectado — probe rapido de 1 dia para confirmar");
                        let probe_label = format!("{}-probe", m.start.format("%Y-%m-%d"));
                        let probe = MonthRange { start: m.start, end: m.start, label: probe_label };
                        let probe_res = tokio::time::timeout(
                            StdDuration::from_secs(120),
                            session.download(&probe, &raw),
                        ).await;
                        // limpiar artefactos del probe
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
        } // if !(enero && had_failed)

        if systemic {
            sys_fail += 1;
            fail.push((m.label.clone(), "servidor intranet sin respuesta (timeout)".to_string()));
            warn!("  {} SYSTEMIC ({}/3)", m.label, sys_fail);
            {
                let mut s = shared.lock().unwrap();
                s.update_progress(pct, format!("{}: servidor sin respuesta ({}/{})", m.label, sys_fail, 3));
            }
            if sys_fail >= 3 {
                let msg = "Servidor intranet caido (3 meses seguidos sin respuesta). Reintente en 30-60 min o en horario laboral.".to_string();
                let mut s = shared.lock().unwrap();
                s.set_phase(CapturePhase::Error, &msg);
                s.update_progress(pct, &msg);
                drop(s);
                warn!("{}", msg);
                progress_handle.abort();
                return Err(anyhow!(msg)); // LockGuard libera solo
            }
            continue;
        }
        sys_fail = 0;

        if !success {
            let mut recovered = false;
            // Atajo: si TODOS los dailies ya estan en disco (ej. tras subida manual), mergear directo sin red
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
                            Ok(ventas) => match insert_ventas(&pool, &ventas).await {
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
            // Enero: granular directo 4 partes (ahorra intento 2 que siempre falla por 31k filas)
            let parts_list: Vec<usize> = if is_enero { vec![4,6,10] } else { vec![2,4,6,10] };
            for parts in parts_list {
                warn!("  {} fallo, probando {} partes", m.label, parts);
                let chunks = split_month_n(m, parts);
                let mut ok_chunks = 0;
                for h in &chunks {
                    let mut h_ok = false;
                    for ha in 1..=2 {
                        let res_h = session.download(h, &raw).await;
                        match res_h {
                            Ok(xls_h) => {
                                let csv_h = raw.join(format!("ventas_{}.csv", h.label));
                                // Usar xls_to_csv (Rust helper que ya resuelve python correcto) en vez de inline python
                                match crate::processor::xls::xls_to_csv(&xls_h, &csv_h) {
                                    Ok(_) if csv_h.exists() && std::fs::metadata(&csv_h).map(|m| m.len() > 500).unwrap_or(false) => {
                                        h_ok = true;
                                        break;
                                    }
                                    Ok(_) => {
                                        warn!("  chunk {} xls_to_csv sin csv valido", h.label);
                                        // no es ok, reintentar
                                    }
                                    Err(e) => {
                                        warn!("  chunk {} xls_to_csv failed: {}", h.label, e);
                                    }
                                }
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
                    let mut parts = Vec::new();
                    for h in &chunks {
                        let part_csv = raw.join(format!("ventas_{}.csv", h.label));
                        if part_csv.exists() { parts.push(part_csv); }
                    }
                    // Merge en Rust puro (sin python)
                    match merge_csvs(&parts, &merged) {
                        Ok(n) => info!("  granular merge rust: {} filas -> {}", n, merged.display()),
                        Err(e) => warn!("  granular merge rust failed: {e:#}"),
                    }
                    // Solo marcar recovered si el merge realmente produjo un CSV valido e insertable
                    let merged_exists = merged.exists() && std::fs::metadata(&merged).map(|m| m.len() > 1000).unwrap_or(false);
                    if merged_exists {
                        match parse_export_csv_with_cross(&merged, &init_pool().await?).await {
                            Ok(ventas) if !ventas.is_empty() => {
                                match init_pool().await {
                                    Ok(pool) => {
                                        match insert_ventas(&pool, &ventas).await {
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
            if !recovered {
                warn!("  {} granular 10 fallo, probando dia-a-dia", m.label);
                let mut cur = m.start;
                let mut day_ok = 0;
                let mut day_total = 0;
                let mut consec_fail = 0usize;
                while cur <= m.end {
                    day_total += 1;
                    let label = cur.format("%Y-%m-%d").to_string();
                    let day_range = MonthRange {
                        start: cur,
                        end: cur,
                        label: label.clone(),
                    };
                    let day_csv = raw.join(format!("ventas_{}.csv", label));
                    let day_xls = raw.join(format!("ventas_{}.xls", label));
                    let mut d_ok = false;
                    if day_csv.exists() && std::fs::metadata(&day_csv).map(|m| m.len() > 1000).unwrap_or(false) {
                        d_ok = true;
                    } else if day_xls.exists() && std::fs::metadata(&day_xls).map(|m| m.len() > 1000).unwrap_or(false) {
                        // Reusar XLS ya descargado (evita 30s por dia) — usar helper Rust
                        if crate::processor::xls::xls_to_csv(&day_xls, &day_csv).is_ok() && day_csv.exists() && std::fs::metadata(&day_csv).map(|m| m.len() > 500).unwrap_or(false) {
                            d_ok = true;
                        }
                    } else {
                    for _ha in 1..=2 {
                        // Reintento con re-login (cookie puede haber expirado a mitad de mes)
                        if _ha == 2 { let _ = session.login().await; }
                        let res_d = session.download(&day_range, &raw).await;
                        match res_d {
                            Ok(xls_d) => {
                                let csv_d = raw.join(format!("ventas_{}.csv", label));
                                if crate::processor::xls::xls_to_csv(&xls_d, &csv_d).is_ok() && csv_d.exists() && std::fs::metadata(&csv_d).map(|m| m.len() > 500).unwrap_or(false) {
                                    d_ok = true;
                                    break;
                                } else {
                                    warn!("  day {} xls_to_csv fallo", label);
                                }
                            }
                            Err(_) => {}
                        }
                        sleep(StdDuration::from_secs(2)).await;
                    }
                    // Si dia muy extenso (12k filas 19/01) falla normal, probar split Art 2 mitades
                    if !d_ok {
                        warn!("  dia {} fail normal, probando split Art", label);
                        match session.download_day_split(&day_range, &raw).await {
                            Ok(_) => {
                                // download_day_split ya deja ventas_YYYY-MM-DD.csv mergeado
                                if day_csv.exists() && std::fs::metadata(&day_csv).map(|m| m.len()>1000).unwrap_or(false) {
                                    d_ok = true;
                                    info!("  dia {} split Art OK", label);
                                }
                            }
                            Err(e) => warn!("  dia {} split Art fail: {}", label, e),
                        }
                    }
                    } // else
                    if d_ok {
                        day_ok += 1;
                        consec_fail = 0;
                    } else {
                        consec_fail += 1;
                        // Backoff adaptativo: fallos seguidos = rate-limit o sesion muerta
                        if consec_fail == 3 {
                            warn!("  3 dias seguidos fallan — backoff 15s + re-login");
                            sleep(StdDuration::from_secs(15)).await;
                            let _ = session.login().await;
                        } else if consec_fail >= 6 {
                            warn!("  {} dias seguidos sin exito — servidor caido, abortando dia-a-dia de {}", consec_fail, m.label);
                            {
                                let mut s = shared.lock().unwrap();
                                s.update_progress(pct, format!("{}: servidor no responde tras {} días seguidos", m.label, consec_fail));
                            }
                            break;
                        }
                    }
                    cur = cur + Duration::days(1);
                    sleep(StdDuration::from_secs(1)).await;
                }
                // Permitir merge parcial: si falta 1-2 días (ej 2024-01-19 sin data) igual mergear 30/31
                if day_ok >= 1 && day_ok + 2 >= day_total && day_total > 0 {
                    let merged = raw.join(format!("ventas_{}.csv", m.label));
                    let mut parts2 = Vec::new();
                    let mut cur2 = m.start;
                    while cur2 <= m.end {
                        let f = raw.join(format!("ventas_{}.csv", cur2.format("%Y-%m-%d")));
                        // Merge parcial tolera dias faltantes
                        if f.exists() { parts2.push(f); }
                        cur2 = cur2 + Duration::days(1);
                    }
                    match merge_csvs(&parts2, &merged) {
                        Ok(n) => info!("  day-a-day merge rust: {} filas -> {}", n, merged.display()),
                        Err(e) => warn!("  day-a-day merge rust failed: {e:#}"),
                    }
                    if merged.exists() {
                        // Insertar directo
                        let pool = init_pool().await?;
                        match parse_export_csv_with_cross(&merged, &pool).await {
                            Ok(ventas) => {
                                match insert_ventas(&pool, &ventas).await {
                                    Ok(n) => info!("  dia-a-dia insert {} filas -> {}", m.label, n),
                                    Err(e) => warn!("  dia-a-dia insert fail {}: {}", m.label, e),
                                }
                            }
                            Err(e) => warn!("  dia-a-dia parse fail {}: {}", m.label, e),
                        }
                        // Detectar dias faltantes para subida manual
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
                            warn!("  {} dias sin capturar (subida manual requerida): {:?}", m.label, faltantes);
                            // Guardar para UI: raw/failed_{mes}.json
                            let fail_path = raw.join(format!("failed_{}.json", m.label));
                            let _ = std::fs::write(&fail_path, serde_json::to_string(&faltantes).unwrap_or_default());
                        } else {
                            let _ = std::fs::remove_file(raw.join(format!("failed_{}.json", m.label)));
                        }
                        recovered = true;
                        let msg_extra = if faltantes.is_empty() { String::new() } else { format!(" (faltan {} dias: {} — sube XLS manual)", faltantes.len(), faltantes.join(",")) };
                        info!("  dia-a-dia OK {} ({}/{}){}", m.label, day_ok, day_total, msg_extra);
                    }
                }
                // Fallback persistente: si dia-a-dia no recupero (ej mes completo falló por rate-limit), probar HTML L0
                if !recovered {
                    warn!("  {} dia-a-dia incompleto, probando fallback HTML L0", m.label);
                    match session.scrape_html(m, &raw).await {
                        Ok(xls) => {
                            let csv = raw.join(format!("ventas_{}.csv", m.label));
                            // scrape_html ya deja CSV, intentar parsear
                            if csv.exists() {
                                let pool = init_pool().await?;
                                match parse_export_csv_with_cross(&csv, &pool).await {
                                    Ok(ventas) if !ventas.is_empty() => {
                                        match insert_ventas(&pool, &ventas).await {
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
                // Generar failed_*.json también en caso de fail total para que UI muestre el día exacto (ej 19/01)
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
                    warn!("  {} fail total — dias faltantes para subida manual: {:?}", m.label, faltantes);
                }
                let detail = if faltantes.is_empty() { "3 intentos + granular 2/4/6/10/dia fallo".to_string() } else { format!("faltan dias {} — sube XLS manual", faltantes.join(",")) };
                fail.push((m.label.clone(), detail));
            }
        } else {
            ok.push(m.label.clone());
        }

        // ETA: estimacion basada en tiempo promedio por item completado
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
            s.update_progress(pct, format!(
                "[{}/{}] {} — quedan ~{}m{}s",
                done_count, total, prefix, eta_m, eta_s
            ));
        }

        if idx + 1 < total {
            // Pacing adaptativo: la fuente es lenta y hace rate-limit.
            // Si el mes recien procesado cargó mucho al server (>60s de render),
            // dejarle una pausa proporcional (elapsed/8, cap 30s) ademas del base 3s.
            let item_secs = t_item.elapsed().as_secs();
            let extra = if item_secs > 60 { (item_secs / 8).min(30) } else { 0 };
            let wait = SLEEP_BETWEEN_MONTHS + extra;
            if extra > 0 {
                info!("  pacing: mes tardo {item_secs}s -> pausa extendida {wait}s (base {} + extra {extra})", SLEEP_BETWEEN_MONTHS);
            }
            sleep(StdDuration::from_secs(wait)).await;
        }
    }

    let elapsed = start_instant.elapsed();
    let elapsed_min = elapsed.as_secs() / 60;
    let elapsed_sec = elapsed.as_secs() % 60;

    // Dedup final
    {
        let mut s = shared.lock().unwrap();
        s.set_phase(CapturePhase::Normalizing, "Eliminando duplicados...");
        s.set_current("dedup");
        s.update_progress(0.92, "Eliminando duplicados...");
    }

    let pool = init_pool().await?;
    let _ = dedup_ventas(&pool).await;

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
