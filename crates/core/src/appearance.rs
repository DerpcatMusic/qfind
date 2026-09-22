//! Shared appearance ids: color Theme, icon pack, and GTK theme name.
//!
//! The TUI owns the full color values; every other frontend maps the same
//! Theme id to its native accent (GTK CSS, Qt palette, WinUI/macOS tint) so
//! one `config.toml` drives all of them.

/// Color Theme ids. Canonical list; see `qfind-tui` `Theme::ALL` for values.
pub const THEMES: &[&str] = &[
    "grok",
    "titanium",
    "catppuccin",
    "gruvbox",
    "dracula",
    "nord",
    "aurora",
];

/// Appearance modes: `custom` = Megaman palette, icon overlay and `custom.css`;
/// `native` = the desktop's GTK/Qt theme untouched (accent from the toolkit).
pub const APPEARANCES: &[&str] = &["custom", "native"];

/// Lowercase known appearance mode, falling back to `custom`.
#[must_use]
pub fn normalize_appearance(name: &str) -> &str {
    if name.trim().eq_ignore_ascii_case("native") {
        "native"
    } else {
        "custom"
    }
}

/// Icon packs: `qfind` (bundled overlay), `system` (desktop theme), `ascii` (text fallback).
pub const ICON_PACKS: &[&str] = &["qfind", "system", "ascii"];

/// GTK theme names offered in Settings. `system` means "don't override".
pub const GTK_THEMES: &[&str] = &["system", "Adwaita", "Adwaita-dark"];

/// Accent hex for a Theme id, used as GTK CSS `@define-color` and Qt/WinUI/macOS tint.
#[must_use]
pub fn accent_for(theme: &str) -> &'static str {
    match normalize_theme(theme) {
        "titanium" => "#00b4ff",
        "catppuccin" => "#89b4fa",
        "gruvbox" => "#8ec07c",
        "dracula" => "#8be9fd",
        "nord" => "#88c0d0",
        "aurora" => "#34d399",
        _ => "#9badff",
    }
}

/// Lowercase known Theme id, falling back to the config default.
#[must_use]
pub fn normalize_theme(name: &str) -> &str {
    let n = name.trim();
    THEMES
        .iter()
        .find(|t| t.eq_ignore_ascii_case(n))
        .copied()
        .unwrap_or("grok")
}

/// Lowercase known icon pack, falling back to the bundled set.
#[must_use]
pub fn normalize_icon_pack(name: &str) -> &str {
    let n = name.trim();
    ICON_PACKS
        .iter()
        .find(|p| p.eq_ignore_ascii_case(n))
        .copied()
        .unwrap_or("qfind")
}

/// `system`/empty means "don't override"; otherwise the verbatim GTK theme name.
#[must_use]
pub fn normalize_gtk_theme(name: &str) -> &str {
    let n = name.trim().trim_matches('"');
    if n.is_empty() || n.eq_ignore_ascii_case("system") {
        "system"
    } else {
        n
    }
}
