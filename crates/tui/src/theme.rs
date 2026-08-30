//! Skins, powerline chips, Nerd glyphs with unicode fallbacks.
//! Palettes follow Oh My Pi / Prime Agent: named tokens, cycleable skins.

use std::sync::OnceLock;
use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const SPIN: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPIN_ASCII: &[&str] = &["|", "/", "-", "\\"];

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
        bg: Color::Rgb(0x0b, 0x0e, 0x14),
        surface: Color::Rgb(0x12, 0x15, 0x1c),
        accent: Color::Rgb(0x5e, 0xea, 0xd4),
        match_fg: Color::Rgb(0xfb, 0x92, 0x3c),
        pink: Color::Rgb(0xf4, 0x72, 0xb6),
        purple: Color::Rgb(0xc0, 0x84, 0xfc),
        sky: Color::Rgb(0x38, 0xbd, 0xf8),
        dim: Color::Rgb(0x64, 0x74, 0x8b),
        text: Color::Rgb(0xe2, 0xe8, 0xf0),
        select_bg: Color::Rgb(0x13, 0x4e, 0x4a),
        zebra: Color::Rgb(0x11, 0x18, 0x27),
        border: Color::Rgb(0x2a, 0x33, 0x44),
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
        dim: Color::Rgb(0x6b, 0x72, 0x80),
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
        dim: Color::Rgb(0x6c, 0x70, 0x86),
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
        dim: Color::Rgb(0x92, 0x83, 0x74),
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
        dim: Color::Rgb(0x62, 0x72, 0xa4),
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
        dim: Color::Rgb(0x4c, 0x56, 0x6a),
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
        dim: Color::Rgb(0x64, 0x74, 0x8b),
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
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| t.id == self.id).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    #[must_use]
    pub fn shimmer(self, t: Duration, i: usize) -> Color {
        let phase = t.as_secs_f32() * 2.4 + (i as f32) * 0.45;
        let w = (phase.sin() * 0.5 + 0.5).clamp(0.0, 1.0) * 0.45;
        lerp(self.accent, self.text, w)
    }

    #[must_use]
    pub fn pulse(self, t: Duration) -> Color {
        let w = ((t.as_secs_f32() * 3.2).sin() * 0.5 + 0.5).clamp(0.0, 1.0) * 0.35;
        lerp(self.select_bg, self.accent, w)
    }
}

pub const BAR: &str = "▎";

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

pub fn sep() -> &'static str {
    if nerd() { "\u{e0b0}" } else { ">" }
}

pub fn icon_prompt() -> &'static str {
    if nerd() { "\u{f0349}" } else { "❯" }
}

pub fn icon_folder() -> &'static str {
    if nerd() { "\u{f024b}" } else { "▸" }
}

pub fn icon_file() -> &'static str {
    if nerd() { "\u{f0214}" } else { "·" }
}

#[must_use]
pub fn spin_frame(t: Duration) -> &'static str {
    let frames = if nerd() { SPIN } else { SPIN_ASCII };
    let ms = if nerd() { 80 } else { 120 };
    frames[(t.as_millis() / ms) as usize % frames.len()]
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

pub fn chip(text: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )
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

pub fn powerline(chips: &[Chip], width: u16, trail: Color) -> Line<'static> {
    let mut spans = Vec::with_capacity(chips.len() * 2 + 1);
    let mut used = 0usize;
    for (i, chip) in chips.iter().enumerate() {
        let body = format!(" {} ", chip.text);
        used += body.chars().count() + 1;
        let next = chips.get(i + 1).map(|c| c.bg).unwrap_or(trail);
        spans.push(Span::styled(
            body,
            Style::new()
                .fg(chip.fg)
                .bg(chip.bg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(sep(), Style::new().fg(chip.bg).bg(next)));
    }
    let pad = (width as usize).saturating_sub(used);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Style::new().bg(trail)));
    }
    Line::from(spans)
}

pub fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color::Rgb(
        ((r + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

pub fn hsl_tile(seed: &str, i: usize, s: f32, l: f32) -> Color {
    let mut h = 17u32;
    for (n, b) in seed.bytes().enumerate() {
        h = h
            .wrapping_mul(31)
            .wrapping_add(u32::from(b))
            .wrapping_add(n as u32);
    }
    h = h.wrapping_add((i as u32).wrapping_mul(47));
    hsl((h % 360) as f32, s, l)
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
