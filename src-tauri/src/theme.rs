//! Design tokens de G360.
//!
//! Generado por `g360 tokens` â€” no editar a mano.
//! Skill: moderno | acento: emerald

/// Tokens del tema oscuro (default).
pub mod dark {
    pub const bg: &str = "#0b1220";
    pub const surface: &str = "#151e2e";
    pub const surface_Alt: &str = "#1c2941";
    pub const overlay: &str = "rgba(11, 18, 32, 0.90)";
    pub const border: &str = "rgba(255, 255, 255, 0.10)";
    pub const border_Strong: &str = "rgba(255, 255, 255, 0.18)";
    pub const text: &str = "#f0f4f8";
    pub const text_Muted: &str = "#94a3b8";
    pub const text_Inverse: &str = "#0b1220";
    pub const accent: &str = "#00d084";
    pub const accent_Strong: &str = "#047857";
    pub const accent_Bright: &str = "#34d399";
    pub const accent_Soft: &str = "rgba(0, 208, 132, 0.12)";
    pub const accent_Hover: &str = "#34d399";
    pub const on_Accent: &str = "#06281c";
    pub const success: &str = "#22c55e";
    pub const warning: &str = "#f59e0b";
    pub const error: &str = "#ef4444";
    pub const info: &str = "#3b82f6";
    pub const glass: &str = "rgba(21, 30, 46, 0.75)";
    pub const blur: &str = "16px";
    pub const radius: &str = "16px";
    pub const radius_Sm: &str = "8px";
    pub const font: &str = "system-ui, -apple-system, 'Segoe UI', sans-serif";
    pub const font_Mono: &str = "'JetBrains Mono', 'Consolas', monospace";
    pub const font_Size: &str = "14px";
    pub const space_Xs: &str = "4px";
    pub const space_Sm: &str = "8px";
    pub const space_Md: &str = "16px";
    pub const space_Lg: &str = "24px";
    pub const space_Xl: &str = "32px";
}

/// Tokens del tema claro.
pub mod light {
    pub const bg: &str = "#ffffff";
    pub const surface: &str = "#f8fafc";
    pub const surface_Alt: &str = "#eef2f7";
    pub const overlay: &str = "rgba(255, 255, 255, 0.92)";
    pub const border: &str = "rgba(0, 0, 0, 0.10)";
    pub const border_Strong: &str = "rgba(0, 0, 0, 0.18)";
    pub const text: &str = "#1e293b";
    pub const text_Muted: &str = "#64748b";
    pub const text_Inverse: &str = "#ffffff";
    pub const accent: &str = "#047857";
    pub const accent_Strong: &str = "#047857";
    pub const accent_Bright: &str = "#00d084";
    pub const accent_Soft: &str = "rgba(0, 208, 132, 0.12)";
    pub const accent_Hover: &str = "#047857";
    pub const on_Accent: &str = "#ffffff";
    pub const success: &str = "#15803d";
    pub const warning: &str = "#b45309";
    pub const error: &str = "#dc2626";
    pub const info: &str = "#0369a1";
    pub const glass: &str = "rgba(255, 255, 255, 0.75)";
    pub const blur: &str = "16px";
    pub const radius: &str = "16px";
    pub const radius_Sm: &str = "8px";
    pub const font: &str = "system-ui, -apple-system, 'Segoe UI', sans-serif";
    pub const font_Mono: &str = "'JetBrains Mono', 'Consolas', monospace";
    pub const font_Size: &str = "14px";
    pub const space_Xs: &str = "4px";
    pub const space_Sm: &str = "8px";
    pub const space_Md: &str = "16px";
    pub const space_Lg: &str = "24px";
    pub const space_Xl: &str = "32px";
}

/// Paleta categorica para series de datos (tema oscuro).
pub mod chart_dark {
    pub const CHART_1: &str = "#06b6d4";
    pub const CHART_2: &str = "#8b5cf6";
    pub const CHART_3: &str = "#0ea5e9";
    pub const CHART_4: &str = "#6366f1";
    pub const CHART_5: &str = "#ec4899";
    pub const CHART_6: &str = "#f59e0b";
    pub const CHART_7: &str = "#ef4444";
    pub const CHART_8: &str = "#f97316";
}

/// Paleta categorica para series de datos (tema claro).
pub mod chart_light {
    pub const CHART_1: &str = "#0891b2";
    pub const CHART_2: &str = "#7c3aed";
    pub const CHART_3: &str = "#0284c7";
    pub const CHART_4: &str = "#4f46e5";
    pub const CHART_5: &str = "#db2777";
    pub const CHART_6: &str = "#b45309";
    pub const CHART_7: &str = "#dc2626";
    pub const CHART_8: &str = "#ea580c";
}

/// Tokens del tema oscuro como pares (nombre, valor).
pub const DARK: &[(&str, &str)] = &[
    ("bg", bg),
    ("surface", surface),
    ("surfaceAlt", surface_Alt),
    ("overlay", overlay),
    ("border", border),
    ("borderStrong", border_Strong),
    ("text", text),
    ("textMuted", text_Muted),
    ("textInverse", text_Inverse),
    ("accent", accent),
    ("accentStrong", accent_Strong),
    ("accentBright", accent_Bright),
    ("accentSoft", accent_Soft),
    ("accentHover", accent_Hover),
    ("onAccent", on_Accent),
    ("success", success),
    ("warning", warning),
    ("error", error),
    ("info", info),
    ("glass", glass),
    ("blur", blur),
    ("radius", radius),
    ("radiusSm", radius_Sm),
    ("font", font),
    ("fontMono", font_Mono),
    ("fontSize", font_Size),
    ("spaceXs", space_Xs),
    ("spaceSm", space_Sm),
    ("spaceMd", space_Md),
    ("spaceLg", space_Lg),
    ("spaceXl", space_Xl),
    ("chart1", CHART_1),
    ("chart2", CHART_2),
    ("chart3", CHART_3),
    ("chart4", CHART_4),
    ("chart5", CHART_5),
    ("chart6", CHART_6),
    ("chart7", CHART_7),
    ("chart8", CHART_8),
];

/// Tokens del tema claro como pares (nombre, valor).
pub const LIGHT: &[(&str, &str)] = &[
    ("bg", bg),
    ("surface", surface),
    ("surfaceAlt", surface_Alt),
    ("overlay", overlay),
    ("border", border),
    ("borderStrong", border_Strong),
    ("text", text),
    ("textMuted", text_Muted),
    ("textInverse", text_Inverse),
    ("accent", accent),
    ("accentStrong", accent_Strong),
    ("accentBright", accent_Bright),
    ("accentSoft", accent_Soft),
    ("accentHover", accent_Hover),
    ("onAccent", on_Accent),
    ("success", success),
    ("warning", warning),
    ("error", error),
    ("info", info),
    ("glass", glass),
    ("blur", blur),
    ("radius", radius),
    ("radiusSm", radius_Sm),
    ("font", font),
    ("fontMono", font_Mono),
    ("fontSize", font_Size),
    ("spaceXs", space_Xs),
    ("spaceSm", space_Sm),
    ("spaceMd", space_Md),
    ("spaceLg", space_Lg),
    ("spaceXl", space_Xl),
    ("chart1", CHART_1),
    ("chart2", CHART_2),
    ("chart3", CHART_3),
    ("chart4", CHART_4),
    ("chart5", CHART_5),
    ("chart6", CHART_6),
    ("chart7", CHART_7),
    ("chart8", CHART_8),
];
