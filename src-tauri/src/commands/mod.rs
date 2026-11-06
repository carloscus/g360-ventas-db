//! Comandos Tauri agrupados por dominio. Cada submódulo expone sus
//! comandos vía `pub use`; `main.rs` importa el módulo con
//! `#[macro_use]` para que `generate_handler!` vea los macros
//! `__cmd__*` generados por `#[tauri::command]`.

pub mod audit;
pub mod capture;
pub mod cartucho;
pub mod config;
pub mod dashboard;
pub mod maintenance;
pub mod months;
pub mod sync;

pub use audit::*;
pub use capture::*;
pub use cartucho::*;
pub use config::*;
pub use dashboard::*;
pub use maintenance::*;

/// Verifica credenciales de intranet con un login real al ERP.
/// Compartido: lo usan commands de meses (delete/import) y maintenance (clear_db).
pub(crate) async fn verify_intranet_credentials() -> Result<String, String> {
    use g360_db_ventas::browser::http::CipsaHttp;
    let http =
        CipsaHttp::new().map_err(|e| format!("Error creando cliente HTTP: {}", e))?;
    http.login()
        .await
        .map_err(|e| format!("Credenciales de intranet invalidas o red no accesible: {}", e))?;
    Ok("OK".to_string())
}
