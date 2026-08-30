//! Startup splash and first-Rebuild setup, in the Oh My Pi / Prime Agent style:
//! skippable intro, rounded chrome, braille spinner, live elapsed.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use qfind_core::{Catalog, Rebuild};
use ratatui::layout::{Alignment, Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::theme::{Theme, chip, spin_frame};

const SPLASH_MS: u64 = 1100;

pub fn play(terminal: &mut DefaultTerminal, theme: &Theme) -> Result<()> {
    if std::env::var_os("QFIND_NOSPLASH").is_some() {
        return Ok(());
    }
    let start = Instant::now();
    let total = Duration::from_millis(SPLASH_MS);
    loop {
        let elapsed = start.elapsed();
        terminal.draw(|f| draw_splash(f, theme, elapsed, total))?;
        if elapsed >= total {
            break;
        }
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => break,
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn rebuild_catalog(
    terminal: &mut DefaultTerminal,
    theme: &Theme,
    rebuild: Rebuild,
) -> Result<Catalog> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(Catalog::rebuild(rebuild));
    });
    let start = Instant::now();
    loop {
        if let Ok(result) = rx.try_recv() {
            return result.context("rebuild Catalog");
        }
        let elapsed = start.elapsed();
        terminal.draw(|f| draw_setup(f, theme, elapsed))?;
        if event::poll(Duration::from_millis(16))? {
            let _ = event::read();
        }
    }
}

fn fill(frame: &mut Frame, theme: &Theme) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg)),
        frame.area(),
    );
}

fn card(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2)).max(24);
    let h = h.min(area.height.saturating_sub(2)).max(7);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn brand_line(theme: &Theme, t: Duration) -> Line<'static> {
    let word = "QFIND";
    let mut spans = Vec::with_capacity(word.len());
    for (i, ch) in word.chars().enumerate() {
        spans.push(Span::styled(
            format!(" {ch} "),
            Style::new()
                .fg(theme.bg)
                .bg(theme.shimmer(t, i))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn draw_splash(frame: &mut Frame, theme: &Theme, elapsed: Duration, total: Duration) {
    fill(frame, theme);
    let popup = card(frame.area(), 42, 9);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.accent))
            .style(Style::new().bg(theme.surface)),
        popup,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .flex(Flex::Center)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(popup.inner(ratatui::layout::Margin::new(2, 1)));

    frame.render_widget(
        Paragraph::new(brand_line(theme, elapsed)).alignment(Alignment::Center),
        inner[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "filename search",
            Style::new().fg(theme.dim).bg(theme.surface),
        ))
        .alignment(Alignment::Center),
        inner[1],
    );
    let ratio = (elapsed.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0);
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(theme.accent).bg(theme.border))
            .ratio(ratio)
            .label(""),
        inner[3],
    );
}

fn setup_step(elapsed: Duration) -> &'static str {
    match elapsed.as_secs() {
        0 => "discovering local Mounts",
        1..=4 => "walking disks  ·  names first, no stat",
        5..=14 => "still walking  ·  large disks take a minute",
        _ => "packing Catalog snapshot",
    }
}

fn draw_setup(frame: &mut Frame, theme: &Theme, elapsed: Duration) {
    fill(frame, theme);
    let popup = card(frame.area(), 56, 11);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.accent))
            .style(Style::new().bg(theme.surface))
            .title(chip("setup", theme.bg, theme.accent)),
        popup,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(popup.inner(ratatui::layout::Margin::new(2, 1)));

    frame.render_widget(
        Paragraph::new(brand_line(theme, elapsed)).alignment(Alignment::Center),
        inner[0],
    );
    let spin = spin_frame(elapsed);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {spin}  "),
                Style::new()
                    .fg(theme.accent)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                setup_step(elapsed),
                Style::new().fg(theme.text).bg(theme.surface),
            ),
        ])),
        inner[2],
    );
    let shift = ((elapsed.as_millis() / 80) % 18) as u16;
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(theme.accent).bg(theme.border))
            .ratio(((f64::from(shift) + 4.0) / 22.0).clamp(0.15, 0.92))
            .label(format!("{}s", elapsed.as_secs())),
        inner[3],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "first Rebuild  ·  Everything-style names",
            Style::new().fg(theme.dim).bg(theme.surface),
        ))
        .alignment(Alignment::Center),
        inner[5],
    );
}
