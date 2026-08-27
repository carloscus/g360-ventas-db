use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Carga variables desde un archivo .env si existen y no están ya definidas.
/// Busca en: cwd, carpeta del ejecutable y hasta 6 niveles hacia arriba
/// (cubre target/release -> raiz del proyecto).
pub fn load_dotenv() {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..=6 {
            match dir.take() {
                Some(d) => {
                    candidates.push(d.join(".env"));
                    dir = d.parent().map(|p| p.to_path_buf());
                }
                None => break,
            }
        }
    }
    for path in candidates {
        if !path.exists() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !k.is_empty() && std::env::var(k).is_err() {
                        std::env::set_var(k, v);
                    }
                }
            }
        }
        break; // solo el primer .env encontrado
    }
}

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

/// Fecha mínima disponible en el intranet (cuando comenzaron los registros)
pub const MIN_AVAILABLE_DATE: chrono::NaiveDate = chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntranetConfig {
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub pass: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub supabase: SupabaseConfig,
    #[serde(default)]
    pub intranet: IntranetConfig,
    /// Responsable del reporte (trazabilidad por corrida)
    #[serde(default)]
    pub generado_por: String,
    #[serde(default = "default_allowed_lines")]
    pub allowed_lines: Vec<String>,
    /// Sincronizar automaticamente al abrir la app
    #[serde(default)]
    pub auto_sync: bool,
    /// Anos de retencion de datos en Supabase (0 = ilimitado)
    #[serde(default)]
    pub data_retention_years: u32,
    /// Ultimo timestamp de sync a Supabase (ISO format, UTC). Para sync incremental.
    #[serde(default)]
    pub last_supabase_sync: Option<String>,
}

impl AppConfig {
    pub fn default_app() -> Self {
        Self {
            supabase: SupabaseConfig::empty(),
            intranet: IntranetConfig::default(),
            generado_por: String::new(),
            allowed_lines: default_allowed_lines(),
            auto_sync: false,
            data_retention_years: 4,
            last_supabase_sync: None,
        }
    }
}

/// Credenciales efectivas del intranet: primero .env, si no, config.json.
pub fn effective_intranet_user() -> String {
    let env = std::env::var("G360_INTRANET_USER").unwrap_or_default();
    if !env.is_empty() { return env; }
    load_config().intranet.user
}

pub fn effective_intranet_pass() -> String {
    let env = std::env::var("G360_INTRANET_PASS").unwrap_or_default();
    if !env.is_empty() { return env; }
    load_config().intranet.pass
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
    // Backup automatico antes de cada cambio (simetria con clear_db)
    if path.exists() {
        let backup_dir = data_dir().join("backup");
        let _ = std::fs::create_dir_all(&backup_dir);
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let backup_path = backup_dir.join(format!("config_{}.json", ts));
        let _ = std::fs::copy(&path, &backup_path);
    }
    // Audit log: registro de quién cambió qué
    let audit_path = logs_dir().join("audit.log");
    let _ = std::fs::create_dir_all(audit_path.parent().unwrap());
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let generado = if cfg.generado_por.is_empty() { "sistema" } else { &cfg.generado_por };
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(&audit_path)
        .and_then(|mut f| writeln!(f, "[{timestamp}] config saved by {generado} — allowed_lines={}, retention={}y, supabase={}",
            cfg.allowed_lines.len(), cfg.data_retention_years, cfg.supabase.is_configured()));
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

pub fn default_allowed_lines() -> Vec<String> {
    vec![
        "01","02","09","11","14","72","73","75","76","77","78","79","85",
        "MA","CA","AD","CB","CC","CD","CE","CF","CG",
    ].into_iter().map(String::from).collect()
}

pub fn effective_allowed_lines() -> Vec<String> {
    let cfg = load_config();
    if cfg.allowed_lines.is_empty() {
        default_allowed_lines()
    } else {
        cfg.allowed_lines
    }
}

pub fn is_allowed_line(id_linea: &str) -> bool {
    if id_linea.len() < 2 {
        return false;
    }
    let suffix = &id_linea[id_linea.len() - 2..];
    effective_allowed_lines().iter().any(|s| s == suffix)
}
