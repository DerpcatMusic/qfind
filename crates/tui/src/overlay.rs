//! Layered TUI windows in the Grok CLI style: help, theme picker, context menu.
//! Drawn last, Esc / click-outside pops the top layer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::theme::Theme;

pub const MENU_ITEMS: &[&str] = &[
    "open",
    "open with",
    "preview",
    "copy path",
    "copy name",
    "copy URI",
    "show in files",
    "open folder",
    "mark / unmark",
    "rename",
    "batch rename",
    "copy selected to…",
    "move selected to…",
    "compress selected…",
    "extract selected…",
    "trash",
    "run action/script…",
];
pub const SETTINGS_APPEARANCE: usize = 6;
pub const SETTINGS_ITEMS: usize = 10;

#[derive(Clone, Debug)]
pub enum Layer {
    Help,
    Location {
        input: String,
    },
    Prompt {
        title: String,
        input: String,
        kind: PromptKind,
    },
    Settings {
        selected: usize,
    },
    Theme {
        selected: usize,
        original: Theme,
    },
    Menu {
        col: u16,
        row: u16,
        idx: usize,
        pick: usize,
    },
    Bookmarks {
        paths: Vec<std::path::PathBuf>,
        selected: usize,
    },
    Review {
        title: String,
        lines: Vec<String>,
        kind: ReviewKind,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Stack {
    layers: Vec<Layer>,
}

impl Stack {
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn top(&self) -> Option<&Layer> {
        self.layers.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut Layer> {
        self.layers.last_mut()
    }

    pub fn push(&mut self, layer: Layer) {
        match layer {
            Layer::Help if self.layers.iter().any(|l| matches!(l, Layer::Help)) => {}
            Layer::Settings { .. }
                if self
                    .layers
                    .iter()
                    .any(|l| matches!(l, Layer::Settings { .. })) => {}
            Layer::Theme { .. } if self.layers.iter().any(|l| matches!(l, Layer::Theme { .. })) => {
            }
            other => self.layers.push(other),
        }
    }

    pub fn pop(&mut self) -> Option<Layer> {
        self.layers.pop()
    }

    pub fn toggle_help(&mut self) {
        if matches!(self.top(), Some(Layer::Help)) {
            self.pop();
        } else {
            self.push(Layer::Help);
        }
    }

    pub fn open_location(&mut self, input: String) {
        self.layers.push(Layer::Location { input });
    }

    pub fn open_prompt(&mut self, title: String, input: String, kind: PromptKind) {
        self.layers.push(Layer::Prompt { title, input, kind });
    }

    pub fn toggle_settings(&mut self) {
        if matches!(self.top(), Some(Layer::Settings { .. })) {
            self.pop();
        } else {
            self.push(Layer::Settings { selected: 0 });
        }
    }

    pub fn toggle_theme(&mut self, current: usize, original: Theme) {
        if matches!(self.top(), Some(Layer::Theme { .. })) {
            self.pop();
        } else {
            self.push(Layer::Theme {
                selected: current,
                original,
            });
        }
    }

    pub fn open_bookmarks(&mut self, paths: Vec<std::path::PathBuf>) {
        self.push(Layer::Bookmarks { paths, selected: 0 });
    }

    pub fn open_review(&mut self, title: String, lines: Vec<String>, kind: ReviewKind) {
        self.push(Layer::Review { title, lines, kind });
    }
}

/// What an input [`Layer::Prompt`] submits.
#[derive(Clone, Debug)]
pub enum PromptKind {
    /// Rename `from` to `<parent>/<input>`.
    Rename { from: std::path::PathBuf },
    /// Create `<parent>/<input>` as a directory.
    Mkdir { parent: std::path::PathBuf },
    /// First field of the GTK-compatible batch rename flow.
    BatchRenameFind { paths: Vec<std::path::PathBuf> },
    /// Second field of the GTK-compatible batch rename flow.
    BatchRenameReplace {
        paths: Vec<std::path::PathBuf>,
        find: String,
    },
    /// Prefix field of the GTK-compatible batch rename flow.
    BatchRenamePrefix {
        paths: Vec<std::path::PathBuf>,
        find: String,
        replace: String,
    },
    /// Suffix field of the GTK-compatible batch rename flow.
    BatchRenameSuffix {
        paths: Vec<std::path::PathBuf>,
        find: String,
        replace: String,
        prefix: String,
    },
    /// Numbering start field of the GTK-compatible batch rename flow.
    BatchRenameStart {
        paths: Vec<std::path::PathBuf>,
        find: String,
        replace: String,
        prefix: String,
        suffix: String,
    },
    /// Destination prompt for copy, move, compression, or extraction.
    Transfer {
        paths: Vec<std::path::PathBuf>,
        action: TransferAction,
    },
    /// Application command used to open one item.
    OpenWith { path: std::path::PathBuf },
    /// Executable action or Nautilus script invoked with selected paths.
    Script { paths: Vec<std::path::PathBuf> },
}

#[derive(Clone, Copy, Debug)]
pub enum TransferAction {
    Copy,
    Move,
    Compress,
    Extract,
}

#[derive(Clone, Debug)]
pub enum ReviewKind {
    BatchRename {
        pairs: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    },
}

impl TransferAction {
    pub fn title(self) -> &'static str {
        match self {
            Self::Copy => "Copy selected to folder",
            Self::Move => "Move selected to folder",
            Self::Compress => "Compress selected (.zip, .7z, .tar.gz)",
            Self::Extract => "Extract archives to folder",
        }
    }
}

pub fn draw(frame: &mut Frame, stack: &Stack, th: Theme, area: Rect, settings: &[(&str, &str)]) {
    for layer in &stack.layers {
        match layer {
            Layer::Help => draw_help(frame, area, th),
            Layer::Location { input } => draw_location(frame, area, th, input),
            Layer::Prompt { title, input, .. } => draw_prompt(frame, area, th, title, input),
            Layer::Settings { selected } => draw_settings(frame, area, th, *selected, settings),
            Layer::Theme { selected, .. } => draw_theme(frame, area, th, *selected),
            Layer::Menu { col, row, pick, .. } => draw_menu(frame, area, th, *col, *row, *pick),
            Layer::Bookmarks { paths, selected } => {
                draw_bookmarks(frame, area, th, paths, *selected)
            }
            Layer::Review { title, lines, .. } => draw_review(frame, area, th, title, lines),
        }
    }
}

/// Hit-test the top layer. `None` means click missed (caller should pop).
pub fn click_top(stack: &mut Stack, x: u16, y: u16, area: Rect) -> Click {
    match stack.top() {
        Some(Layer::Help) => {
            let r = help_rect(area, CLICK_HELP_ROWS);
            if close_hit(r, x, y) {
                stack.pop();
                Click::Closed
            } else if r.contains(ratatui::layout::Position::new(x, y)) {
                Click::Ignore
            } else {
                stack.pop();
                Click::Closed
            }
        }
        Some(Layer::Location { .. }) => {
            let r = location_rect(area);
            if close_hit(r, x, y) || !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                Click::Closed
            } else {
                Click::Ignore
            }
        }
        Some(Layer::Prompt { .. }) => {
            let r = location_rect(area);
            if close_hit(r, x, y) || !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                Click::Closed
            } else {
                Click::Ignore
            }
        }
        Some(Layer::Settings { .. }) => {
            let r = settings_rect(area);
            if close_hit(r, x, y) || !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                return Click::Closed;
            }
            settings_index_at(y.saturating_sub(r.y.saturating_add(1)))
                .map_or(Click::Ignore, Click::Settings)
        }
        Some(Layer::Theme { .. }) => {
            let r = theme_rect(area);
            if close_hit(r, x, y) || !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                return Click::Closed;
            }
            let i = y.saturating_sub(r.y.saturating_add(1)) as usize;
            if i < Theme::ALL.len() {
                Click::Theme(i)
            } else {
                Click::Ignore
            }
        }
        Some(Layer::Menu { col, row, .. }) => {
            let r = menu_rect(*col, *row, area);
            if !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                return Click::Closed;
            }
            let i = y.saturating_sub(r.y.saturating_add(1)) as usize;
            if i < MENU_ITEMS.len() {
                Click::Menu(i)
            } else {
                Click::Ignore
            }
        }
        Some(Layer::Bookmarks { paths, .. }) => {
            let r = bookmarks_rect(area, paths.len());
            if close_hit(r, x, y) || !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                return Click::Closed;
            }
            let i = y.saturating_sub(r.y.saturating_add(1)) as usize;
            if i < paths.len() {
                Click::Bookmark(i)
            } else {
                Click::Ignore
            }
        }
        Some(Layer::Review { lines, .. }) => {
            let r = review_rect(area, lines.len());
            if close_hit(r, x, y) || !r.contains(ratatui::layout::Position::new(x, y)) {
                stack.pop();
                return Click::Closed;
            }
            if y == r.bottom().saturating_sub(2) {
                Click::ReviewAccept
            } else {
                Click::Ignore
            }
        }
        None => Click::Miss,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Click {
    Miss,
    Ignore,
    Closed,
    Theme(usize),
    Settings(usize),
    Menu(usize),
    Bookmark(usize),
    ReviewAccept,
}

/// Row count used for click hit-testing; kept in sync with `draw_help`.
const CLICK_HELP_ROWS: usize = 33;

fn help_rect(area: Rect, rows: usize) -> Rect {
    let w = 58.min(area.width.saturating_sub(2)).max(2);
    let h = ((rows + 2) as u16)
        .min(area.height.saturating_sub(2))
        .max(2);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn location_rect(area: Rect) -> Rect {
    let w = 82.min(area.width.saturating_sub(2)).max(2);
    let h = 3.min(area.height.saturating_sub(2)).max(2);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + 2.min(area.height.saturating_sub(h));
    Rect::new(x, y, w, h)
}

fn theme_rect(area: Rect) -> Rect {
    let w = 46.min(area.width.saturating_sub(2)).max(2);
    let h = (Theme::ALL.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(2);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn settings_rect(area: Rect) -> Rect {
    let w = 54.min(area.width.saturating_sub(2)).max(2);
    let h = (SETTINGS_ITEMS as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(2);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn settings_index_at(line: u16) -> Option<usize> {
    match line {
        1..=6 => Some(line as usize - 1),
        8..=11 => Some(line as usize - 2),
        _ => None,
    }
}

pub fn menu_rect(col: u16, row: u16, area: Rect) -> Rect {
    let w = 24u16;
    let h = MENU_ITEMS.len() as u16 + 2;
    let mut x = col.min(area.x + area.width.saturating_sub(w));
    let mut y = row.min(area.y + area.height.saturating_sub(h));
    if x < area.x {
        x = area.x;
    }
    if y < area.y {
        y = area.y;
    }
    Rect::new(x, y, w, h)
}

fn bookmarks_rect(area: Rect, count: usize) -> Rect {
    let w = 64.min(area.width.saturating_sub(2)).max(2);
    let h = count.saturating_add(2).min(u16::MAX as usize) as u16;
    let h = h.min(area.height.saturating_sub(2)).max(2);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn review_rect(area: Rect, count: usize) -> Rect {
    let w = 90.min(area.width.saturating_sub(2)).max(2);
    let h = count.saturating_add(4).min(u16::MAX as usize) as u16;
    let h = h
        .min(area.height.saturating_sub(2))
        .max(4.min(area.height.max(4)));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn close_hit(area: Rect, x: u16, y: u16) -> bool {
    y == area.y && x >= area.right().saturating_sub(4)
}

fn window<'a>(title: &'a str, th: Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th.accent))
        .style(Style::new().bg(th.surface))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(ratatui::layout::Alignment::Left)
}

fn shortcut(key: &str, label: &str, th: Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {key} "),
            Style::new()
                .fg(th.bg)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{label} "), Style::new().fg(th.dim).bg(th.surface)),
    ]
}

fn clear_popup(frame: &mut Frame, popup: Rect, th: Theme) {
    if popup.right() < frame.area().right() {
        frame.render_widget(
            Block::default().style(Style::new().bg(th.select_bg)),
            Rect::new(popup.right(), popup.y.saturating_add(1), 1, popup.height),
        );
    }
    if popup.bottom() < frame.area().bottom() {
        frame.render_widget(
            Block::default().style(Style::new().bg(th.select_bg)),
            Rect::new(popup.x.saturating_add(1), popup.bottom(), popup.width, 1),
        );
    }
    frame.render_widget(Clear, popup);
}

fn draw_help(frame: &mut Frame, area: Rect, th: Theme) {
    let actions = [
        ("Space", "preview focused item"),
        ("Tab", "switch Search / Results focus"),
        ("Enter", "open or enter folder"),
        ("Delete", "trash focused / marked (undo: Ctrl+Z)"),
        ("Insert", "mark / unmark focused row"),
        ("Ctrl+A", "mark all / clear marks"),
        ("F2", "rename focused item"),
        ("F7", "new folder here"),
        ("Ctrl+Z", "undo last file operation"),
        ("Ctrl+T / W", "new / close tab"),
        ("Ctrl+] / [", "next / previous tab"),
        ("F4", "dual-pane browser"),
        ("Ctrl+L", "open location"),
        ("F6", "map: size, file count, off"),
        ("F8", "appearance & behavior"),
        ("+ / −", "grid density"),
        ("↑↓", "navigate"),
        ("←→", "grid or browser pane"),
        ("Alt+←→", "browser back / forward"),
        ("Alt+Z", "zebra stripes"),
        ("Mouse drag", "drop a result into another app"),
        ("Ctrl+O", "show in files"),
        ("Ctrl+Y", "copy path"),
        ("Ctrl+C / Shift", "copy paths / names"),
        ("Ctrl+S", "save archive / cycle sort"),
        ("Ctrl+G", "search everywhere"),
        ("F5", "refresh files and catalog"),
        ("Ctrl+B", "pin current folder"),
        ("Ctrl+Shift+B", "open pinned folders"),
        ("F10", "context menu"),
        ("F9 / Ctrl+P", "projects, storage, Git, tasks"),
        ("Ctrl+M", "change match mode"),
        ("right-click", "actions"),
    ];
    let popup = help_rect(area, actions.len());
    clear_popup(frame, popup, th);
    let visible = popup.height.saturating_sub(2) as usize;
    let text = actions
        .into_iter()
        .take(visible)
        .map(|(key, label)| Line::from(shortcut(key, label, th)))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(
                window("Shortcuts", th)
                    .title_top(
                        Line::from(Span::styled("[×]", Style::new().fg(th.dim))).right_aligned(),
                    )
                    .title_bottom(Line::from(shortcut("Esc", "close", th))),
            ),
        popup,
    );
}

fn draw_location(frame: &mut Frame, area: Rect, th: Theme, input: &str) {
    let popup = location_rect(area);
    clear_popup(frame, popup, th);
    let width = popup.width.saturating_sub(4) as usize;
    let skip = input.chars().count().saturating_sub(width);
    let visible = input.chars().skip(skip).collect::<String>();
    frame.render_widget(
        Paragraph::new(visible.clone())
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(
                window("Location", th)
                    .title_top(
                        Line::from(Span::styled("[×]", Style::new().fg(th.dim))).right_aligned(),
                    )
                    .title_bottom(Line::from(
                        [shortcut("Ctrl+U", "clear", th), shortcut("Enter", "go", th)].concat(),
                    )),
            ),
        popup,
    );
    frame.set_cursor_position(ratatui::layout::Position::new(
        popup
            .x
            .saturating_add(1)
            .saturating_add(visible.chars().count() as u16),
        popup.y.saturating_add(1),
    ));
}

fn draw_prompt(frame: &mut Frame, area: Rect, th: Theme, title: &str, input: &str) {
    let popup = location_rect(area);
    clear_popup(frame, popup, th);
    let width = popup.width.saturating_sub(4) as usize;
    let skip = input.chars().count().saturating_sub(width);
    let visible = input.chars().skip(skip).collect::<String>();
    frame.render_widget(
        Paragraph::new(visible.clone())
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(
                window(title, th)
                    .title_top(
                        Line::from(Span::styled("[×]", Style::new().fg(th.dim))).right_aligned(),
                    )
                    .title_bottom(Line::from(
                        [
                            shortcut("Ctrl+U", "clear", th),
                            shortcut("Enter", "apply", th),
                        ]
                        .concat(),
                    )),
            ),
        popup,
    );
    frame.set_cursor_position(ratatui::layout::Position::new(
        popup
            .x
            .saturating_add(1)
            .saturating_add(visible.chars().count() as u16),
        popup.y.saturating_add(1),
    ));
}

fn draw_theme(frame: &mut Frame, area: Rect, th: Theme, selected: usize) {
    let popup = theme_rect(area);
    clear_popup(frame, popup, th);
    let lines: Vec<Line> = Theme::ALL
        .iter()
        .enumerate()
        .map(|(i, skin)| {
            let on = i == selected;
            let bg = if on { th.select_bg } else { th.surface };
            Line::from(vec![
                Span::styled(
                    if on { " • " } else { "   " },
                    Style::new().fg(th.accent).bg(bg),
                ),
                Span::styled(
                    format!("{:<13}", skin.id),
                    Style::new()
                        .fg(if on { th.text } else { th.dim })
                        .bg(bg)
                        .add_modifier(if on {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled("◆ ", Style::new().fg(skin.accent).bg(bg)),
                Span::styled("◆ ", Style::new().fg(skin.match_fg).bg(bg)),
                Span::styled("◆ ", Style::new().fg(skin.purple).bg(bg)),
                Span::styled("◆ ", Style::new().fg(skin.sky).bg(bg)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(
                window("Themes", th)
                    .title_top(
                        Line::from(Span::styled("[×]", Style::new().fg(th.dim))).right_aligned(),
                    )
                    .title_bottom(Line::from(
                        [
                            shortcut("↑↓", "try", th),
                            shortcut("Enter", "keep", th),
                            shortcut("Esc", "back", th),
                        ]
                        .concat(),
                    )),
            ),
        popup,
    );
}

fn draw_settings(
    frame: &mut Frame,
    area: Rect,
    th: Theme,
    selected: usize,
    settings: &[(&str, &str)],
) {
    let popup = settings_rect(area);
    clear_popup(frame, popup, th);
    let mut lines = vec![Line::from(Span::styled(
        "  Appearance",
        Style::new().fg(th.dim).bg(th.surface),
    ))];
    let setting_line = |(i, (label, value)): (usize, &(&str, &str))| {
        let on = i == selected;
        let bg = if on { th.select_bg } else { th.surface };
        Line::from(vec![
            Span::styled(
                if on { "  • " } else { "    " },
                Style::new().fg(th.accent).bg(bg),
            ),
            Span::styled(
                format!("{label:<16}"),
                Style::new()
                    .fg(if on { th.text } else { th.dim })
                    .bg(bg)
                    .add_modifier(if on {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(format!("{value:>27}"), Style::new().fg(th.accent).bg(bg)),
        ])
    };
    lines.extend(
        settings[..SETTINGS_APPEARANCE]
            .iter()
            .enumerate()
            .map(&setting_line),
    );
    lines.push(Line::from(Span::styled(
        "  Catalog",
        Style::new().fg(th.dim).bg(th.surface),
    )));
    lines.extend(
        settings[SETTINGS_APPEARANCE..]
            .iter()
            .enumerate()
            .map(|(i, setting)| setting_line((i + SETTINGS_APPEARANCE, setting))),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(
                window("Settings", th)
                    .title_top(
                        Line::from(Span::styled("[×]", Style::new().fg(th.dim))).right_aligned(),
                    )
                    .title_bottom(Line::from(
                        [
                            shortcut("↑↓", "select", th),
                            shortcut("←→", "change", th),
                            shortcut("Esc", "close", th),
                        ]
                        .concat(),
                    )),
            ),
        popup,
    );
}

fn draw_menu(frame: &mut Frame, area: Rect, th: Theme, col: u16, row: u16, pick: usize) {
    let popup = menu_rect(col, row, area);
    clear_popup(frame, popup, th);
    let lines: Vec<Line> = MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let on = i == pick;
            let bg = if on { th.select_bg } else { th.surface };
            Line::from(Span::styled(
                format!(" {item} "),
                Style::new().fg(if on { th.accent } else { th.text }).bg(bg),
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(window("Actions", th).title_bottom(Line::from(shortcut("Esc", "close", th)))),
        popup,
    );
}

fn draw_bookmarks(
    frame: &mut Frame,
    area: Rect,
    th: Theme,
    paths: &[std::path::PathBuf],
    selected: usize,
) {
    let popup = bookmarks_rect(area, paths.len());
    clear_popup(frame, popup, th);
    let lines: Vec<Line> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let on = i == selected;
            let bg = if on { th.select_bg } else { th.surface };
            Line::from(Span::styled(
                format!(" {} ", path.display()),
                Style::new().fg(if on { th.accent } else { th.text }).bg(bg),
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(window("Places", th).title_bottom(Line::from(shortcut("Enter", "open", th)))),
        popup,
    );
}

fn draw_review(frame: &mut Frame, area: Rect, th: Theme, title: &str, lines: &[String]) {
    let popup = review_rect(area, lines.len());
    clear_popup(frame, popup, th);
    let mut body = lines
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                line.clone(),
                Style::new().fg(th.text).bg(th.surface),
            ))
        })
        .collect::<Vec<_>>();
    body.push(Line::from(""));
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::new().fg(th.text).bg(th.surface))
            .block(
                window(title, th)
                    .title_top(
                        Line::from(Span::styled("[×]", Style::new().fg(th.dim))).right_aligned(),
                    )
                    .title_bottom(Line::from(
                        [
                            shortcut("Esc", "cancel", th),
                            shortcut("Enter", "apply", th),
                        ]
                        .concat(),
                    )),
            ),
        popup,
    );
}
