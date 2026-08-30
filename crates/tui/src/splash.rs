//! Honest first-run indexing state.

use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result};
use qfind_core::{Catalog, Rebuild};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::theme::Theme;

pub fn rebuild_catalog(
    terminal: &mut DefaultTerminal,
    theme: &Theme,
    rebuild: Rebuild,
) -> Result<Catalog> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(Catalog::rebuild(rebuild));
    });
    terminal.draw(|frame| draw_setup(frame, theme))?;
    rx.recv()
        .context("Catalog rebuild worker stopped")?
        .context("rebuild Catalog")
}

fn fill(frame: &mut Frame, theme: &Theme) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg)),
        frame.area(),
    );
}

fn draw_setup(frame: &mut Frame, theme: &Theme) {
    fill(frame, theme);
    let area = frame.area();
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .split(area)[1];

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Building the first Catalog",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Center),
        ratatui::layout::Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new("This only happens once")
            .alignment(Alignment::Center)
            .style(Style::new().fg(theme.dim).bg(theme.bg)),
        ratatui::layout::Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
}
