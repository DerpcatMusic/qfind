//! Oh My Posh powerline chips + Grok CLI canvas. Nerd glyphs with unicode fallbacks.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub const BG: Color = Color::Rgb(0x0b, 0x0e, 0x14);
pub const SURFACE: Color = Color::Rgb(0x12, 0x15, 0x1c);
pub const ACCENT: Color = Color::Rgb(0x5e, 0xea, 0xd4);
pub const MATCH: Color = Color::Rgb(0xfb, 0x92, 0x3c);
pub const PINK: Color = Color::Rgb(0xf4, 0x72, 0xb6);
pub const PURPLE: Color = Color::Rgb(0xc0, 0x84, 0xfc);
pub const SKY: Color = Color::Rgb(0x38, 0xbd, 0xf8);
pub const DIM: Color = Color::Rgb(0x64, 0x74, 0x8b);
pub const TEXT: Color = Color::Rgb(0xe2, 0xe8, 0xf0);
pub const SELECT_BG: Color = Color::Rgb(0x13, 0x4e, 0x4a);
pub const ZEBRA: Color = Color::Rgb(0x11, 0x18, 0x27);
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
