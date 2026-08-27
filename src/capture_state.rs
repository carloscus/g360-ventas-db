use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub enum CapturePhase {
    Idle,
    CheckingLock,
    Downloading,
    Parsing,
    Normalizing,
    Uploading,
    Done,
    Error,
}

impl fmt::Display for CapturePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapturePhase::Idle => write!(f, "idle"),
            CapturePhase::CheckingLock => write!(f, "checking_lock"),
            CapturePhase::Downloading => write!(f, "downloading"),
            CapturePhase::Parsing => write!(f, "parsing"),
            CapturePhase::Normalizing => write!(f, "normalizing"),
            CapturePhase::Uploading => write!(f, "uploading"),
            CapturePhase::Done => write!(f, "done"),
            CapturePhase::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProgressState {
    pub phase: CapturePhase,
    pub message: String,
    pub progress: f32,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub current_item: String,
    /// Estimacion de segundos restantes (None = aun sin datos suficientes)
    pub eta_secs: Option<u64>,
}

impl ProgressState {
    pub fn new() -> Self {
        Self {
            phase: CapturePhase::Idle,
            message: "Listo".into(),
            progress: 0.0,
            started_at: None,
            finished_at: None,
            current_item: String::new(),
            eta_secs: None,
        }
    }

    pub fn set_start(&mut self, msg: &str) {
        self.phase = CapturePhase::Downloading;
        self.message = msg.to_string();
        self.progress = 0.0;
        self.started_at = Some(now_secs());
        self.finished_at = None;
        self.current_item.clear();
    }

    pub fn set_phase<P: AsRef<str>>(&mut self, phase: CapturePhase, msg: P) {
        self.phase = phase;
        self.message = msg.as_ref().to_string();
        if self.phase == CapturePhase::Done || self.phase == CapturePhase::Error {
            self.finished_at = Some(now_secs());
        }
    }

    pub fn update_progress<P: AsRef<str>>(&mut self, pct: f32, msg: P) {
        self.progress = pct;
        let msg = msg.as_ref();
        if !msg.is_empty() {
            self.message = msg.to_string();
        }
    }

    pub fn set_current(&mut self, item: &str) {
        self.current_item = item.into();
    }
}

impl Default for ProgressState {
    fn default() -> Self { Self::new() }
}

pub type SharedProgress = Arc<Mutex<ProgressState>>;

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}
