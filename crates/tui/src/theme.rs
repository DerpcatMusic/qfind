//! Qfind's compact visual system and Nerd glyph fallbacks.

use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub bg: Color,
    pub surface: Color,
    pub accent: Color,
    pub match_fg: Color,
    pub pink: Color,
    pub purple: Color,
    pub sky: Color,
    pub dim: Color,
    pub text: Color,
    pub select_bg: Color,
    pub zebra: Color,
    pub border: Color,
}

impl Theme {
    pub const GROK: Self = Self {
        id: "grok",
        bg: Color::Rgb(0x0a, 0x0b, 0x10),
        surface: Color::Rgb(0x12, 0x14, 0x1c),
        accent: Color::Rgb(0x9b, 0xad, 0xff),
        match_fg: Color::Rgb(0xd1, 0x9f, 0xff),
        pink: Color::Rgb(0xea, 0x8c, 0xb5),
        purple: Color::Rgb(0xa9, 0x8b, 0xe8),
        sky: Color::Rgb(0x9b, 0xc5, 0xee),
        dim: Color::Rgb(0x7d, 0x81, 0x90),
        text: Color::Rgb(0xf1, 0xf2, 0xf7),
        select_bg: Color::Rgb(0x20, 0x25, 0x38),
        zebra: Color::Rgb(0x0a, 0x0b, 0x10),
        border: Color::Rgb(0x2b, 0x30, 0x40),
    };

    /// Oh My Pi default dark skin.
    pub const TITANIUM: Self = Self {
        id: "titanium",
        bg: Color::Rgb(0x0f, 0x12, 0x16),
        surface: Color::Rgb(0x15, 0x18, 0x20),
        accent: Color::Rgb(0x00, 0xb4, 0xff),
        match_fg: Color::Rgb(0xff, 0xb3, 0x47),
        pink: Color::Rgb(0xf0, 0xc0, 0x40),
        purple: Color::Rgb(0xd4, 0xc0, 0x90),
        sky: Color::Rgb(0x00, 0xff, 0x88),
        dim: Color::Rgb(0x9c, 0xa3, 0xaf),
        text: Color::Rgb(0xe8, 0xec, 0xf4),
        select_bg: Color::Rgb(0x00, 0x50, 0x70),
        zebra: Color::Rgb(0x15, 0x18, 0x20),
        border: Color::Rgb(0x2a, 0x30, 0x38),
    };

    pub const CATPPUCCIN: Self = Self {
        id: "catppuccin",
        bg: Color::Rgb(0x1e, 0x1e, 0x2e),
        surface: Color::Rgb(0x18, 0x18, 0x25),
        accent: Color::Rgb(0x89, 0xb4, 0xfa),
        match_fg: Color::Rgb(0xfa, 0xb3, 0x87),
        pink: Color::Rgb(0xf5, 0xc2, 0xe7),
        purple: Color::Rgb(0xcb, 0xa6, 0xf7),
        sky: Color::Rgb(0x89, 0xdc, 0xeb),
        dim: Color::Rgb(0xa6, 0xad, 0xc8),
        text: Color::Rgb(0xcd, 0xd6, 0xf4),
        select_bg: Color::Rgb(0x45, 0x47, 0x5a),
        zebra: Color::Rgb(0x18, 0x18, 0x25),
        border: Color::Rgb(0x31, 0x32, 0x44),
    };

    pub const GRUVBOX: Self = Self {
        id: "gruvbox",
        bg: Color::Rgb(0x28, 0x28, 0x28),
        surface: Color::Rgb(0x3c, 0x38, 0x36),
        accent: Color::Rgb(0x8e, 0xc0, 0x7c),
        match_fg: Color::Rgb(0xfe, 0x80, 0x19),
        pink: Color::Rgb(0xd3, 0x86, 0x9b),
        purple: Color::Rgb(0xd3, 0x86, 0x9b),
        sky: Color::Rgb(0x83, 0xa5, 0x98),
        dim: Color::Rgb(0xa8, 0x99, 0x84),
        text: Color::Rgb(0xeb, 0xdb, 0xb2),
        select_bg: Color::Rgb(0x50, 0x49, 0x45),
        zebra: Color::Rgb(0x32, 0x30, 0x2f),
        border: Color::Rgb(0x50, 0x49, 0x45),
    };

    pub const DRACULA: Self = Self {
        id: "dracula",
        bg: Color::Rgb(0x28, 0x2a, 0x36),
        surface: Color::Rgb(0x21, 0x22, 0x2c),
        accent: Color::Rgb(0x8b, 0xe9, 0xfd),
        match_fg: Color::Rgb(0xff, 0xb8, 0x6c),
        pink: Color::Rgb(0xff, 0x79, 0xc6),
        purple: Color::Rgb(0xbd, 0x93, 0xf9),
        sky: Color::Rgb(0x50, 0xfa, 0x7b),
        dim: Color::Rgb(0x9a, 0xa3, 0xc7),
        text: Color::Rgb(0xf8, 0xf8, 0xf2),
        select_bg: Color::Rgb(0x44, 0x47, 0x5a),
        zebra: Color::Rgb(0x21, 0x22, 0x2c),
        border: Color::Rgb(0x44, 0x47, 0x5a),
    };

    pub const NORD: Self = Self {
        id: "nord",
        bg: Color::Rgb(0x2e, 0x34, 0x40),
        surface: Color::Rgb(0x3b, 0x42, 0x52),
        accent: Color::Rgb(0x88, 0xc0, 0xd0),
        match_fg: Color::Rgb(0xeb, 0xcb, 0x8b),
        pink: Color::Rgb(0xb4, 0x8e, 0xad),
        purple: Color::Rgb(0xb4, 0x8e, 0xad),
        sky: Color::Rgb(0x81, 0xa1, 0xc1),
        dim: Color::Rgb(0xae, 0xb8, 0xc8),
        text: Color::Rgb(0xec, 0xef, 0xf4),
        select_bg: Color::Rgb(0x43, 0x4c, 0x5e),
        zebra: Color::Rgb(0x3b, 0x42, 0x52),
        border: Color::Rgb(0x4c, 0x56, 0x6a),
    };

    pub const AURORA: Self = Self {
        id: "aurora",
        bg: Color::Rgb(0x0b, 0x12, 0x20),
        surface: Color::Rgb(0x11, 0x18, 0x27),
        accent: Color::Rgb(0x34, 0xd3, 0x99),
        match_fg: Color::Rgb(0xfb, 0xbf, 0x24),
        pink: Color::Rgb(0xf4, 0x72, 0xb6),
        purple: Color::Rgb(0xa7, 0x8b, 0xfa),
        sky: Color::Rgb(0x22, 0xd3, 0xee),
        dim: Color::Rgb(0x94, 0xa3, 0xb8),
        text: Color::Rgb(0xe2, 0xe8, 0xf0),
        select_bg: Color::Rgb(0x06, 0x4e, 0x3b),
        zebra: Color::Rgb(0x0f, 0x17, 0x2a),
        border: Color::Rgb(0x1e, 0x29, 0x3b),
    };

    pub const ALL: &[Self] = &[
        Self::GROK,
        Self::TITANIUM,
        Self::CATPPUCCIN,
        Self::GRUVBOX,
        Self::DRACULA,
        Self::NORD,
        Self::AURORA,
    ];

    #[must_use]
    pub fn parse(name: &str) -> Self {
        let n = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|t| t.id.eq_ignore_ascii_case(n))
            .unwrap_or(Self::TITANIUM)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| t.id == self.id).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    #[must_use]
    pub fn map_tile(self, i: usize) -> Color {
        let color = [self.accent, self.purple, self.sky, self.pink][i % 4];
        lerp(self.surface, color, 0.46)
    }

    #[must_use]
    pub fn glow(self, t: f32) -> Color {
        if t < 0.5 {
            lerp(self.accent, self.purple, t * 2.0)
        } else {
            lerp(self.purple, self.pink, (t - 0.5) * 2.0)
        }
    }
}

pub struct Chip {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
}

impl Chip {
    pub fn new(text: impl Into<String>, fg: Color, bg: Color) -> Self {
        Self {
            text: text.into(),
            fg,
            bg,
        }
    }

    fn cols(&self) -> usize {
        self.text.chars().count() + 3
    }
}

pub fn nerd() -> bool {
    static NERD: OnceLock<bool> = OnceLock::new();
    *NERD.get_or_init(|| {
        if std::env::var_os("QFIND_ASCII").is_some() {
            return false;
        }
        let term = std::env::var("TERM").unwrap_or_default();
        if term == "dumb" || term == "linux" {
            return false;
        }
        let loc = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default();
        loc.is_empty() || loc.to_ascii_lowercase().contains("utf")
    })
}

pub fn icon_prompt() -> &'static str {
    if nerd() { "\u{f0349}" } else { "⌕" }
}

pub fn icon_folder() -> &'static str {
    if nerd() { "\u{f024b}" } else { "▸" }
}

pub fn icon_file() -> &'static str {
    if nerd() { "\u{f0214}" } else { "·" }
}

pub fn icon_for(path: &Path, is_dir: bool) -> &'static str {
    if is_dir {
        return icon_folder();
    }
    if !nerd() {
        return icon_file();
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => "\u{f03e}",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aiff" => "\u{f001}",
        "mp4" | "mkv" | "mov" | "avi" | "webm" => "\u{f03d}",
        "zip" | "gz" | "xz" | "7z" | "rar" | "tar" => "\u{f410}",
        "pdf" => "\u{f1c1}",
        "rs" => "\u{e7a8}",
        "toml" | "yaml" | "yml" | "json" | "ini" | "conf" => "\u{e615}",
        "sh" | "bash" | "zsh" | "fish" => "\u{f489}",
        "c" | "h" | "cpp" | "hpp" | "js" | "jsx" | "ts" | "tsx" | "py" | "go" | "java" | "html"
        | "css" => "\u{f121}",
        "md" | "txt" | "rst" | "doc" | "docx" => "\u{f15c}",
        _ => icon_file(),
    }
}

#[must_use]
pub fn compact(n: u32) -> String {
    match n {
        n if n >= 1_000_000 => format!("{}M", n / 1_000_000),
        n if n >= 10_000 => format!("{}k", n / 1_000),
        n if n >= 1_000 => format!("{:.1}k", f64::from(n) / 1000.0),
        n => n.to_string(),
    }
}

pub fn fit_chips(chips: Vec<Chip>, width: u16) -> Vec<Chip> {
    let max = width as usize;
    let mut out = Vec::with_capacity(chips.len());
    let mut used = 0usize;
    for (i, chip) in chips.into_iter().enumerate() {
        let w = chip.cols();
        if i > 0 && used + w > max {
            break;
        }
        used += w;
        out.push(chip);
    }
    out
}

pub fn toolbar(chips: &[Chip], width: u16, trail: Color) -> Line<'static> {
    let mut spans = Vec::with_capacity(chips.len() * 2 + 1);
    let mut used = 0usize;
    for (i, chip) in chips.iter().enumerate() {
        let body = format!(" {} ", chip.text);
        used += body.chars().count();
        spans.push(Span::styled(
            body,
            Style::new()
                .fg(chip.fg)
                .bg(chip.bg)
                .add_modifier(Modifier::BOLD),
        ));
        if i + 1 < chips.len() {
            spans.push(Span::styled(" ", Style::new().bg(trail)));
            used += 1;
        }
    }
    let pad = (width as usize).saturating_sub(used);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Style::new().bg(trail)));
    }
    Line::from(spans)
}

fn lerp(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let t = t.clamp(0.0, 1.0);
    Color::Rgb(
        (f32::from(ar) + (f32::from(br) - f32::from(ar)) * t).round() as u8,
        (f32::from(ag) + (f32::from(bg) - f32::from(ag)) * t).round() as u8,
        (f32::from(ab) + (f32::from(bb) - f32::from(ab)) * t).round() as u8,
    )
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_and_fallback() {
        assert_eq!(Theme::parse("titanium").id, "titanium");
        assert_eq!(Theme::parse("CATPPUCCIN").id, "catppuccin");
        assert_eq!(Theme::parse("nope").id, "titanium");
    }

    #[test]
    fn cycle_visits_every_skin() {
        let mut t = Theme::GROK;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..Theme::ALL.len() {
            seen.insert(t.id);
            t = t.next();
        }
        assert_eq!(seen.len(), Theme::ALL.len());
        assert_eq!(t.id, Theme::GROK.id);
    }
}
