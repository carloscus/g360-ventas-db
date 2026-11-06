//! Estado compartido: flags globales, caches con TTL y helpers de fase.
//! Los comandos lo importan con `use crate::state::*;`.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use g360_db_ventas::capture_state::{CapturePhase, ProgressState, SharedProgress};

use crate::types::DashboardStats;

pub(crate) static CAPTURE_STATE: LazyLock<SharedProgress> =
    LazyLock::new(|| std::sync::Arc::new(std::sync::Mutex::new(ProgressState::new())));

/// Flag para abortar captura en curso. Se consulta en el loop de capture.
pub(crate) static CAPTURE_ABORT: LazyLock<Arc<AtomicBool>> =
    LazyLock::new(|| Arc::new(AtomicBool::new(false)));

/// Cache TTL para las consultas pesadas del dashboard (GROUP BY sobre 2.8M filas).
/// El UI hace polling cada ~2s: sin cache, la query tarda 4-24s y satura el pool.
pub(crate) static DASHBOARD_CACHE: LazyLock<std::sync::Mutex<Option<(Instant, DashboardStats)>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));
pub(crate) const DASHBOARD_CACHE_TTL_SECS: u64 = 60;

/// Cache TTL para los conteos crudos de get_health (`COUNT(*)` sobre 2.8M filas).
pub(crate) static HEALTH_CACHE: LazyLock<std::sync::Mutex<Option<(Instant, i64, String, String)>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));
pub(crate) const HEALTH_CACHE_TTL_SECS: u64 = 60;

/// Cache de la info de Supabase (min/max, filas, documentos). Evita pegarle al
/// proyecto en cada poll: se refresca cada 60s.
pub(crate) static SUPABASE_INFO_CACHE: LazyLock<
    std::sync::Mutex<Option<(Instant, g360_db_ventas::processor::uploader::SupabaseInfo)>>,
> = LazyLock::new(|| std::sync::Mutex::new(None));
pub(crate) const SUPABASE_INFO_TTL_SECS: u64 = 60;

/// Invalida las caches del dashboard tras una captura/upload para refrescar el UI.
pub(crate) fn invalidate_dashboard_caches() {
    *DASHBOARD_CACHE.lock().unwrap() = None;
    *HEALTH_CACHE.lock().unwrap() = None;
    *SUPABASE_INFO_CACHE.lock().unwrap() = None;
}

pub(crate) fn set_state(state: &str, msg: &str, progress: f32) {
    let mut s = CAPTURE_STATE.lock().unwrap();
    s.phase = match state {
        "idle" => CapturePhase::Idle,
        "uploading" => CapturePhase::Uploading,
        _ => CapturePhase::Downloading,
    };
    s.message = msg.to_string();
    s.progress = progress;
    if state == "syncing" || state == "uploading" {
        s.started_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        s.finished_at = None;
    } else if state == "idle" {
        s.finished_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
    }
}

// ── Helpers de estado ──────────────────────────────────────────────────
/// Map internal CapturePhase to frontend-compatible state string
pub(crate) fn phase_to_state(phase: &CapturePhase) -> &'static str {
    match phase {
        CapturePhase::Idle => "idle",
        CapturePhase::CheckingLock => "syncing",
        CapturePhase::Downloading => "syncing",
        CapturePhase::Parsing => "syncing",
        CapturePhase::Normalizing => "syncing",
        CapturePhase::Uploading => "uploading",
        CapturePhase::Done => "idle",
        CapturePhase::Error => "error",
    }
}
