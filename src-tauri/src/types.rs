//! Tipos compartidos (respuestas de comandos y structs de estado).

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DashboardStats {
    pub total_records: i64,
    pub total_sales: f64,
    pub total_clients: i64,
    pub total_skus: i64,
    pub months: Vec<MonthStats>,
}

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct MonthStats {
    pub mes_ref: String,
    pub rows: i64,
    pub sales: f64,
    pub clients: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CaptureStatus {
    pub state: String,
    pub phase: String,
    pub message: String,
    pub progress: f32,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub current_item: String,
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthStats {
    pub total_records: i64,
    pub min_date: String,
    pub max_date: String,
    pub last_capture: String,
    pub db_size_mb: f64,
    pub status: String,
    pub db_path: String,
    pub db_exists: bool,
    // Versionado de contrato
    pub contracto_version: String,
    pub contracto_compatibil: bool,
    // Cobertura dimensional
    pub dim_cliente: usize,
    pub dim_vendedor: usize,
    pub dim_articulo: usize,
    pub dim_documento: usize,
    pub dim_linea: usize,
    // Checksums
    pub checksum_months: usize,
    pub total_months: usize,
    pub foreign_keys_on: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    pub supabase_url: String,
    pub supabase_key: String,
    pub supabase_service_role_configured: bool,
    pub supabase_table: String,
    pub supabase_configured: bool,
    pub supabase_enabled: bool,
    pub intranet_user: String,
    pub intranet_pass: String,
    pub generado_por: String,
    pub allowed_lines: String,
    pub auto_sync: bool,
    pub app_retention_years: u32,
    pub supabase_retention_years: u32,
    pub supabase_retention_days: u32,
    pub last_supabase_sync: Option<String>,
    pub auto_daily_capture: bool,
    pub capture_times: Vec<String>,
    pub db_path: String,
    pub backup_reminder_days: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletenessInfo {
    pub mes_ref: String,
    pub dias_esperados: i32,
    pub dias_con_data: i32,
    pub dailies_en_disco: i32,
    pub filas: i64,
    pub faltan: Vec<String>,
    pub estado: String, // OK / PARCIAL / VACIO
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FailedDayInfo {
    pub mes: String,
    pub dias_faltantes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreviewResult {
    pub file_name: String,
    pub total_rows: usize,
    pub insertable_rows: usize,
    pub tipo_desglose: std::collections::HashMap<String, usize>,
    pub referencia_status: std::collections::HashMap<String, usize>,
    pub preview_rows: Vec<Vec<String>>,
    pub warnings: Vec<String>,
}
