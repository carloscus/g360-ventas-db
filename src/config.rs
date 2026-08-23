use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const INTRANET_URL: &str = "http://intranet.cipsa.com.pe/intranetcipsa";
pub const LOGIN_URL: &str = "http://intranet.cipsa.com.pe/intranetcipsa/login.aspx";
pub const APPS_URL: &str = "http://intranet.cipsa.com.pe/intranetcipsa/aplicaionesA.aspx";
pub const STATS_URL: &str = "http://intranet.cipsa.com.pe/ESTADISTICASVENTAS/Default.aspx";

/// Credenciales de acceso al intranet leídas desde variables de entorno.
/// La captura requiere que `G360_INTRANET_USER` / `G360_INTRANET_PASS` estén definidas.
pub fn intranet_username() -> String {
    std::env::var("G360_INTRANET_USER").unwrap_or_default()
}

pub fn intranet_password() -> String {
    std::env::var("G360_INTRANET_PASS").unwrap_or_default()
}

pub const SUPABASE_TABLE: &str = "ventas";
pub const DEFAULT_SUPABASE_URL: &str = "https://TU_SUPABASE.supabase.co";
pub const DEFAULT_SUPABASE_KEY: &str = "TU_ANON_KEY";

pub const MONTHS_BACK: u32 = 12;
pub const SLEEP_BETWEEN_MONTHS: u64 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupabaseConfig {
    pub url: String,
    pub key: String,
}

impl SupabaseConfig {
    pub fn empty() -> Self {
        Self {
            url: DEFAULT_SUPABASE_URL.to_string(),
            key: DEFAULT_SUPABASE_KEY.to_string(),
        }
    }
    pub fn is_configured(&self) -> bool {
        !self.url.is_empty()
            && !self.url.contains("TU_SUPABASE")
            && !self.key.is_empty()
            && !self.key.contains("TU_ANON_KEY")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub supabase: SupabaseConfig,
}

impl AppConfig {
    pub fn default_app() -> Self {
        Self {
            supabase: SupabaseConfig::empty(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("g360-db-ventas")
        .join("data")
}

pub fn raw_dir() -> PathBuf {
    data_dir().join("raw")
}

pub fn db_path() -> PathBuf {
    data_dir().join("historial.db")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn config_file_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn load_config() -> AppConfig {
    let path = config_file_path();
    if path.exists() {
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str(&s) {
                return cfg;
            }
        }
    }
    AppConfig::default_app()
}

pub fn save_config(cfg: &AppConfig) -> anyhow::Result<()> {
    let path = config_file_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let s = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, s)?;
    Ok(())
}

pub fn get_supabase_url() -> String {
    load_config().supabase.url.clone()
}

pub fn get_supabase_key() -> String {
    load_config().supabase.key.clone()
}

pub const ALLOWED_LINE_SUFFIXES: &[&str] = &[
    "01", "02", "09", "11", "14", "72", "73", "75", "76", "77", "78", "79", "85", "MA", "CA", "AD",
    "CB", "CC", "CD", "CE", "CF", "CG",
];

pub fn is_allowed_line(id_linea: &str) -> bool {
    if id_linea.len() < 2 {
        return false;
    }
    let suffix = &id_linea[id_linea.len() - 2..];
    ALLOWED_LINE_SUFFIXES.contains(&suffix)
}
