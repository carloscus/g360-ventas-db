use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

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
pub const MIN_AVAILABLE_DATE: chrono::NaiveDate =
    chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupabaseConfig {
    pub url: String,
    /// Anon key: lectura publica (frontend, apps downstream). Sin permisos de escritura.
    pub key: String,
    /// Service role key: solo lectura interna del backend Rust. Omite RLS, permite writes.
    /// NUNCA exponer al frontend ni hardcodear en el repositorio.
    #[serde(default)]
    pub service_role_key: String,
}

impl SupabaseConfig {
    pub fn empty() -> Self {
        Self {
            url: DEFAULT_SUPABASE_URL.to_string(),
            key: DEFAULT_SUPABASE_KEY.to_string(),
            service_role_key: String::new(),
        }
    }
    pub fn is_configured(&self) -> bool {
        !self.url.is_empty()
            && !self.url.contains("TU_SUPABASE")
            && !self.key.is_empty()
            && !self.key.contains("TU_ANON_KEY")
    }
}

/// Clave usada por el backend Rust para writes (insert/update/delete).
/// Retorna el service role key si existe, sino el anon key (fallback legado).
pub fn get_supabase_service_key() -> String {
    let cfg = load_config();
    if !cfg.supabase.service_role_key.is_empty() {
        cfg.supabase.service_role_key
    } else {
        // fallback: usar anon key si no hay service role configurado
        cfg.supabase.key
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
    /// Años de retención para apps derivadas (datos activos)
    #[serde(default = "default_app_retention")]
    pub app_retention_years: u32,
    /// Años de retención para Supabase (buffer NC/ND) — legacy, usado si days=0
    #[serde(default = "default_supabase_retention")]
    pub supabase_retention_years: u32,
    /// Retención Supabase en días (precisión sub-anual, ej: 1277 = 3.5 años). 0 = usar years.
    #[serde(default)]
    pub supabase_retention_days: u32,
    /// Ultimo timestamp de sync a Supabase (ISO format, UTC). Para sync incremental.
    #[serde(default)]
    pub last_supabase_sync: Option<String>,
    /// Master switch: si es false, la app NO toca Supabase (100% local).
    /// Default true para no cambiar el comportamiento en configs existentes.
    #[serde(default = "default_true")]
    pub supabase_enabled: bool,
    /// Captura automática diaria (false = manual)
    #[serde(default)]
    pub auto_daily_capture: bool,
    /// Horarios de captura diaria (formato "HH:MM", timezone local)
    #[serde(default = "default_capture_times")]
    pub capture_times: Vec<String>,
    /// Recordatorio de backup local (días). 0 = deshabilitado.
    #[serde(default = "default_backup_reminder")]
    pub backup_reminder_days: u32,
    /// Backups de BD a conservar (rotación: se eliminan los más viejos al crear uno nuevo).
    #[serde(default = "default_backup_keep")]
    pub backup_keep: u32,
    /// Cartuchos exportados a conservar (.db/.zip/.manifest; se eliminan los más viejos).
    #[serde(default = "default_cartucho_keep")]
    pub cartucho_keep: u32,
}

fn default_backup_reminder() -> u32 {
    7
}
fn default_backup_keep() -> u32 {
    7
}
fn default_cartucho_keep() -> u32 {
    3
}

fn default_app_retention() -> u32 {
    3
}
fn default_supabase_retention() -> u32 {
    4
}
fn default_true() -> bool {
    true
}
fn default_capture_times() -> Vec<String> {
    vec!["12:00".to_string(), "20:00".to_string()]
}

impl AppConfig {
    /// Retención efectiva de Supabase en días (0 = ilimitada).
    /// Prioriza `supabase_retention_days` (precisión sub-anual); si es 0, usa years * 365.
    pub fn supabase_retention_days_effective(&self) -> u32 {
        if self.supabase_retention_days > 0 {
            self.supabase_retention_days
        } else {
            self.supabase_retention_years * 365
        }
    }
}

impl AppConfig {
    pub fn default_app() -> Self {
        Self {
            supabase: SupabaseConfig::empty(),
            intranet: IntranetConfig::default(),
            generado_por: String::new(),
            allowed_lines: default_allowed_lines(),
            auto_sync: false,
            app_retention_years: 3,
            supabase_retention_years: 4,
            supabase_retention_days: 0,
            last_supabase_sync: None,
            supabase_enabled: true,
            auto_daily_capture: false,
            capture_times: default_capture_times(),
            backup_reminder_days: default_backup_reminder(),
            backup_keep: default_backup_keep(),
            cartucho_keep: default_cartucho_keep(),
        }
    }
}

/// Credenciales efectivas del intranet: primero .env, si no, config.json.
pub fn effective_intranet_user() -> String {
    let env = std::env::var("G360_INTRANET_USER").unwrap_or_default();
    if !env.is_empty() {
        return env;
    }
    load_config().intranet.user
}

pub fn effective_intranet_pass() -> String {
    let env = std::env::var("G360_INTRANET_PASS").unwrap_or_default();
    if !env.is_empty() {
        return env;
    }
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

pub fn export_dir() -> PathBuf {
    data_dir().join("export")
}

pub fn config_file_path() -> PathBuf {
    data_dir().join("config.json")
}

/// Ruta del config encriptado (DPAPI).
pub fn config_enc_path() -> PathBuf {
    data_dir().join("config.enc")
}

/// Encripta datos con Windows DPAPI (solo esta PC/usuario puede desencriptar).
#[cfg(target_os = "windows")]
fn dpapi_encrypt(data: &str) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        let result = CryptProtectData(&in_blob, None, None, None, None, 0, &mut out_blob);
        if result.is_ok() {
            let encrypted =
                std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
            let _ = LocalFree(windows::Win32::Foundation::HLOCAL(
                out_blob.pbData as *mut _,
            ));
            Some(encrypted)
        } else {
            None
        }
    }
}

/// Desencripta datos con Windows DPAPI.
#[cfg(target_os = "windows")]
fn dpapi_decrypt(data: &[u8]) -> Option<String> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        let result = CryptUnprotectData(&in_blob, None, None, None, None, 0, &mut out_blob);
        if result.is_ok() {
            let decrypted = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
            let s = String::from_utf8_lossy(decrypted).to_string();
            let _ = LocalFree(windows::Win32::Foundation::HLOCAL(
                out_blob.pbData as *mut _,
            ));
            Some(s)
        } else {
            None
        }
    }
}

// Fallbacks para no-Windows (no usados en producción pero permiten compilar)
#[cfg(not(target_os = "windows"))]
fn dpapi_encrypt(data: &str) -> Option<Vec<u8>> {
    Some(data.as_bytes().to_vec())
}
#[cfg(not(target_os = "windows"))]
fn dpapi_decrypt(data: &[u8]) -> Option<String> {
    Some(String::from_utf8_lossy(data).to_string())
}

/// Carga config. Primero intenta .enc (DPAPI), luego .json (legacy/migración).
pub fn load_config() -> AppConfig {
    // 1. Intentar config.enc (encriptado)
    let enc_path = config_enc_path();
    if enc_path.exists() {
        if let Ok(bytes) = std::fs::read(&enc_path) {
            if let Some(json) = dpapi_decrypt(&bytes) {
                if let Ok(cfg) = serde_json::from_str(&json) {
                    return cfg;
                }
            }
        }
        // Si .enc existe pero falla el decrypt, probablemente es de otra PC
        // → caer al fallback
    }

    // 2. Intentar config.json (legacy plano) + migrar automáticamente
    let plain_path = config_file_path();
    if plain_path.exists() {
        if let Ok(s) = std::fs::read_to_string(&plain_path) {
            if let Ok(cfg) = serde_json::from_str(&s) {
                // Migrar a encriptado automáticamente
                let _ = save_config_enc(&cfg);
                // Renombrar .json → .json.bak (backup del plano original)
                let bak_path = data_dir().join("config.json.bak");
                let _ = std::fs::rename(&plain_path, &bak_path);
                return cfg;
            }
        }
    }

    AppConfig::default_app()
}

/// Guarda config encriptada con DPAPI.
pub fn save_config_enc(cfg: &AppConfig) -> anyhow::Result<()> {
    let enc_path = config_enc_path();
    if let Some(p) = enc_path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    match dpapi_encrypt(&json) {
        Some(encrypted) => {
            std::fs::write(&enc_path, encrypted)?;
            Ok(())
        }
        None => anyhow::bail!("DPAPI encrypt falló"),
    }
}

/// Indica si es el primer inicio (no hay config .enc ni .json).
pub fn is_first_run() -> bool {
    !config_enc_path().exists() && !config_file_path().exists()
}

pub fn save_config(cfg: &AppConfig) -> anyhow::Result<()> {
    // Siempre guardar encriptado
    save_config_enc(cfg)?;

    // Audit log: registro de quién cambió qué
    let audit_path = logs_dir().join("audit.log");
    let _ = std::fs::create_dir_all(audit_path.parent().unwrap());
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let generado = if cfg.generado_por.is_empty() {
        "sistema"
    } else {
        &cfg.generado_por
    };
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(&audit_path)
        .and_then(|mut f| writeln!(f, "[{timestamp}] config saved by {generado} — allowed_lines={}, retention={}y, supabase={}",
            cfg.allowed_lines.len(), cfg.supabase_retention_years, cfg.supabase.is_configured()));
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
        "01", "02", "09", "11", "14", "72", "73", "75", "76", "77", "78", "79", "81", "85", "99",
        "MA", "CA", "AD", "CB", "CC", "CD", "CE", "CF", "CG",
    ]
    .into_iter()
    .map(String::from)
    .collect()
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
    // Ahora id_linea es de 2 chars sin prefijo (ej: MA, 01, 78)
    effective_allowed_lines().contains(&id_linea.to_uppercase())
}

/// WHERE SQL obligatorio que aplica el filtro de líneas al subir a Supabase (raw-first).
/// La BD local guarda TODO el export del ERP; Supabase solo recibe lo "vital":
///   1) filas con línea permitida (sufijo de 2 chars en allowlist, igual que is_allowed_line), o
///   2) NC/ND cuya FACTURA DE VENTA (F01) existe en la BD local con línea permitida.
/// El gate F01% alinea el filtro con el resolver cross-mes anterior y con las vistas
/// (vw_devoluciones / vw_nc_totales / vw_facturas_disponibles): una NC que referencia
/// una BDI (compra) es devolución de compra, no venta -> se queda solo en local.
/// Sustituye al filtro en-parser: cambios de allowlist se aplican con solo re-subir,
/// sin re-capturar. El lookup de NC/ND es global (cubre facturas de otros meses).
/// Sanitización estricta [A-Z0-9]{2} para prevenir inyección vía config.json.
/// Fail-closed: allowlist vacía/inválida -> "1 = 0" (no se sube nada).
pub fn supabase_line_filter_sql() -> String {
    let list = effective_allowed_lines()
        .iter()
        .filter(|l| l.len() == 2 && l.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|l| format!("'{}'", l.to_uppercase()))
        .collect::<Vec<_>>()
        .join(", ");
    if list.is_empty() {
        return "1 = 0".to_string();
    }
    format!(
        "(id_linea IN ({list}) OR ((tpo_doc LIKE '%NCR%' OR tpo_doc LIKE '%NDB%') \
         AND EXISTS (SELECT 1 FROM ventas f \
          WHERE f.serie_doc = ventas.factura_ref_serie \
            AND f.nro_doc = ventas.factura_ref_nro \
            AND (f.tpo_doc LIKE 'F01%' OR f.tpo_doc = 'F01') \
            AND f.id_linea IN ({list}))))"
    )
}

/// Combina un WHERE opcional (sync/retención: "" o "WHERE ...") con el filtro
/// obligatorio de líneas. Usar en todo SELECT/COUNT que calcule qué filas
/// se suben a Supabase (upload, dry-run, validación, stats_cache).
pub fn with_line_filter(extra_where: &str) -> String {
    let f = supabase_line_filter_sql();
    let extra = extra_where.trim();
    if extra.is_empty() {
        format!("WHERE {}", f)
    } else if extra.len() >= 5 && extra[..5].eq_ignore_ascii_case("where") {
        format!("{} AND {}", extra, f)
    } else {
        format!("WHERE {} AND {}", extra, f)
    }
}
