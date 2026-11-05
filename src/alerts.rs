//! Sistema de alertas estructuradas para la captura.
//!
//! Cada error se clasifica por severidad y lleva una acción sugerida.
//! Al final de una corrida, las alertas se resumen en un solo bloque.
//!
//! Uso:
//!   let mut alerts = AlertCollector::new();
//!   alerts.critical("2020-04", "DB locked", "Reintentar con importxls");
//!   alerts.warning("2020-09", "Timeout servidor", "Verificar intranet");
//!   alerts.report();  // imprime resumen al final

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    /// Mes no capturado — requiere intervención manual
    Critical,
    /// Captura parcial o con warnings — revisar
    Warning,
    /// Informativo — no requiere acción
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Critical => write!(f, "🔴 CRÍTICA"),
            Severity::Warning => write!(f, "🟡 WARNING"),
            Severity::Info => write!(f, "🔵 INFO"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Alert {
    pub severity: Severity,
    pub target: String, // mes, archivo, o componente afectado
    pub error: String,  // qué falló
    pub action: String, // qué hacer para resolverlo
    pub auto_fix: bool, // si el retry automático lo resuelve
}

impl fmt::Display for Alert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] {}\n  → Acción: {}\n  → Auto-fix: {}",
            self.severity,
            self.target,
            self.error,
            self.action,
            if self.auto_fix {
                "Sí (retry automático)"
            } else {
                "No — requiere intervención"
            }
        )
    }
}

/// Recolector de alertas para una corrida de captura.
pub struct AlertCollector {
    alerts: Vec<Alert>,
}

impl AlertCollector {
    pub fn new() -> Self {
        Self { alerts: Vec::new() }
    }

    /// Registra una alerta crítica (mes no capturado).
    pub fn critical(&mut self, target: &str, error: &str, action: &str) {
        self.alerts.push(Alert {
            severity: Severity::Critical,
            target: target.to_string(),
            error: error.to_string(),
            action: action.to_string(),
            auto_fix: false,
        });
    }

    /// Registra una warning (captura parcial, timeout, etc.).
    pub fn warning(&mut self, target: &str, error: &str, action: &str) {
        self.alerts.push(Alert {
            severity: Severity::Warning,
            target: target.to_string(),
            error: error.to_string(),
            action: action.to_string(),
            auto_fix: false,
        });
    }

    /// Registra una alerta que el retry automático puede resolver.
    pub fn auto_fixable(&mut self, target: &str, error: &str) {
        self.alerts.push(Alert {
            severity: Severity::Warning,
            target: target.to_string(),
            error: error.to_string(),
            action: "Retry automático en progreso".to_string(),
            auto_fix: true,
        });
    }

    /// Registra info (no cuenta como alerta).
    pub fn info(&mut self, target: &str, msg: &str) {
        self.alerts.push(Alert {
            severity: Severity::Info,
            target: target.to_string(),
            error: msg.to_string(),
            action: "Ninguna".to_string(),
            auto_fix: false,
        });
    }

    /// Imprime resumen al final de la corrida.
    pub fn report(&self) {
        if self.alerts.is_empty() {
            tracing::info!("✅ Sin alertas — captura limpia");
            return;
        }

        let criticals: Vec<_> = self
            .alerts
            .iter()
            .filter(|a| a.severity == Severity::Critical)
            .collect();
        let warnings: Vec<_> = self
            .alerts
            .iter()
            .filter(|a| a.severity == Severity::Warning)
            .collect();
        let infos: Vec<_> = self
            .alerts
            .iter()
            .filter(|a| a.severity == Severity::Info)
            .collect();

        tracing::info!("══════ REPORTE DE CAPTURA ══════");
        tracing::info!(
            "🔴 {} críticas | 🟡 {} warnings | 🔵 {} info",
            criticals.len(),
            warnings.len(),
            infos.len()
        );

        for a in &criticals {
            tracing::error!("{}", a);
        }
        for a in &warnings {
            tracing::warn!("{}", a);
        }
        for a in &infos {
            tracing::info!("{}", a);
        }

        if !criticals.is_empty() {
            tracing::error!("⚠️  Hay alertas críticas — revisar antes de distribuir cartucho");
        }
    }

    /// Devuelve true si hay alertas críticas (para exit code).
    pub fn has_criticals(&self) -> bool {
        self.alerts.iter().any(|a| a.severity == Severity::Critical)
    }

    /// Serializa a JSON (para webhook futuro).
    pub fn to_json(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .alerts
            .iter()
            .map(|a| {
                serde_json::json!({
                    "severity": format!("{}", a.severity),
                    "target": a.target,
                    "error": a.error,
                    "action": a.action,
                    "auto_fix": a.auto_fix,
                })
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({ "alerts": items })).unwrap_or_default()
    }
}

impl Default for AlertCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_no_panics() {
        let c = AlertCollector::new();
        assert!(!c.has_criticals());
        c.report(); // no debe panic
    }

    #[test]
    fn critical_detection() {
        let mut c = AlertCollector::new();
        c.critical("2020-04", "DB locked", "importxls");
        c.warning("2020-09", "Timeout", "Reintentar");
        assert!(c.has_criticals());
    }

    #[test]
    fn auto_fix_not_critical() {
        let mut c = AlertCollector::new();
        c.auto_fixable("2025-01", "DB locked");
        assert!(!c.has_criticals());
    }

    #[test]
    fn json_serialization() {
        let mut c = AlertCollector::new();
        c.critical("2020-04", "DB locked", "importxls");
        let json = c.to_json();
        assert!(json.contains("2020-04"));
        assert!(json.contains("CRÍTICA"));
    }
}
