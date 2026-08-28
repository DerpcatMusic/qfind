use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use qfind_core::{
    Catalog, Config, DateAge, FileClass, Hit, HitRef, MatchMode, Scope, SearchOpts, Sort, Surface,
    Zoom, default_snapshot_path, folder_weights, squarify,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::{DefaultTerminal, Frame};

mod theme;
use theme::{
    ACCENT, BG, Chip, DIM, MATCH, PINK, PURPLE, SELECT_BG, SKY, SURFACE, TEXT, ZEBRA, chip,
    compact, fit_chips, hsl_tile, icon_file, icon_folder, icon_prompt, powerline,
};

const MAX_ROWS: usize = 2_000;

/// Open the Qfind TUI. Rebuilds the Catalog on first launch if missing.
pub fn run() -> Result<()> {
    let snapshot = default_snapshot_path();
    let catalog = if snapshot.exists() {
        Catalog::open(&snapshot).with_context(|| format!("open {}", snapshot.display()))?
    } else {
        eprintln!("first launch: rebuilding Catalog (this can take a minute)…");
        Catalog::rebuild(qfind_core::Rebuild::new(&snapshot))
            .with_context(|| format!("rebuild {}", snapshot.display()))?
    };
    let warm = catalog.clone();
    thread::spawn(move || warm.warm());
    let mut app = App::new(catalog);
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

struct Row {
    name: String,
    path: PathBuf,
    is_dir: bool,
    indices: Vec<u32>,
}

impl Row {
    fn from_hit(hit: Hit<'_>) -> Self {
        Self {
            name: hit.name().to_string(),
            path: hit.path(),
            is_dir: hit.is_dir(),
            indices: hit.indices().to_vec(),
        }
    }
}

type SearchMsg = (u64, Result<Vec<Row>, String>);

struct App {
    catalog: Catalog,
    query: String,
    rows: Vec<Row>,
    selected: usize,
    status: String,
    help: bool,
    dragging: bool,
    scope: Scope,
    class: FileClass,
    sort: Sort,
    match_mode: MatchMode,
    date: DateAge,
    zebra: bool,
    zoom: Zoom,
    surface: Surface,
    show_weight: bool,
    hits_area: Rect,
    list_start: usize,
    seq: u64,
    inbox: Option<mpsc::Receiver<SearchMsg>>,
    dirty: bool,
    last_edit: Instant,
}

impl App {
    fn new(catalog: Catalog) -> Self {
        let cfg = Config::load();
        Self {
            status: format!(
                "{} folders · {} files",
                catalog.folder_count(),
                catalog.file_count()
            ),
            catalog,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            help: false,
            dragging: false,
            scope: Scope::All,
            class: FileClass::All,
            sort: Sort::Score,
            match_mode: cfg.match_mode,
            date: DateAge::Any,
            zebra: cfg.zebra,
            zoom: Zoom::new(cfg.zoom),
            surface: Surface::Auto,
            show_weight: cfg.weight_map,
            hits_area: Rect::default(),
            list_start: 0,
            seq: 0,
            inbox: None,
            dirty: true,
            last_edit: Instant::now(),
        }
    }

    fn opts(&self) -> SearchOpts {
        SearchOpts {
            scope: self.scope,
            class: self.class,
            sort: self.sort,
            date: self.date,
            limit: MAX_ROWS,
            highlight: true,
            match_mode: self.match_mode,
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_edit = Instant::now();
    }

    fn kick_search(&mut self) {
        self.dirty = false;
        self.seq += 1;
        let seq = self.seq;
        let catalog = self.catalog.clone();
        let q = self.query.clone();
        let opts = self.opts();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let rows = catalog
                .search_with(&q, opts)
                .map(|hits| hits.iter().map(Row::from_hit).collect())
                .map_err(|e| e.to_string());
            let _ = tx.send((seq, rows));
        });
        self.inbox = Some(rx);
        self.status = "searching…".into();
    }

    fn poll_inbox(&mut self) {
        let Some(rx) = &self.inbox else {
            return;
        };
        let Ok((seq, result)) = rx.try_recv() else {
            return;
        };
        self.inbox = None;
        if seq != self.seq {
            return;
        }
        match result {
            Ok(rows) => {
                self.selected = 0;
                self.status = format!(
                    "{} hits  ·  {} folders · {} files",
                    rows.len(),
                    self.catalog.folder_count(),
                    self.catalog.file_count()
                );
                self.rows = rows;
            }
            Err(err) => {
                self.status = err;
                self.rows.clear();
            }
        }
    }

    fn open_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let path = row.path.clone();
        match Command::new("xdg-open").arg(&path).spawn() {
            Ok(_) => self.status = format!("opened {}", path.display()),
            Err(err) => self.status = format!("xdg-open: {err}"),
        }
    }

    fn drag_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            self.status = "nothing selected to drag".into();
            return;
        };
        let path = row.path.clone();
        self.dragging = true;
        match Command::new("ripdrag")
            .args(["-x", "-i", "-s", "48"])
            .arg(&path)
            .spawn()
        {
            Ok(_) => {
                self.status = format!("drag  {}", path.display());
            }
            Err(err) => {
                self.dragging = false;
                self.status = format!("ripdrag missing ({err}) — pacman/cargo install ripdrag");
            }
        }
    }

    fn preview_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let path = row.path.clone();
        if Command::new("sushi").arg(&path).spawn().is_ok() {
            self.status = format!("preview  {}", path.display());
            return;
        }
        match Command::new("xdg-open").arg(&path).spawn() {
            Ok(_) => self.status = format!("opened {}", path.display()),
            Err(err) => self.status = format!("preview: {err}"),
        }
    }

    fn reveal_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let path = row.path.clone();
        let uri = format!("file://{}", path.display());
        let dbus = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.FileManager1",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("['{uri}']"),
                "",
            ])
            .spawn();
        if dbus.is_ok() {
            self.status = format!("show in files  {}", path.display());
            return;
        }
        let folder = if row.is_dir {
            path.clone()
        } else {
            path.parent().unwrap_or(path.as_path()).to_path_buf()
        };
        match Command::new("xdg-open").arg(&folder).spawn() {
            Ok(_) => self.status = format!("folder  {}", folder.display()),
            Err(err) => self.status = format!("reveal: {err}"),
        }
    }

    fn copy_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let text = row.path.to_string_lossy().into_owned();
        let ok = pipe_copy("wl-copy", &[], &text)
            || pipe_copy("xclip", &["-selection", "clipboard"], &text);
        self.status = if ok {
            format!("copied  {text}")
        } else {
            "copy: install wl-copy or xclip".into()
        };
    }
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        app.poll_inbox();
        if app.dirty && app.last_edit.elapsed() >= Duration::from_millis(50) {
            app.kick_search();
        }
        terminal.draw(|f| draw(f, app))?;
        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(app, key) {
                    break;
                }
            }
            Event::Mouse(m) => handle_mouse(app, m),
            _ => {}
        }
    }
    Ok(())
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    let pos = Position::new(m.column, m.row);
    match m.kind {
        MouseEventKind::ScrollUp if m.modifiers.contains(KeyModifiers::CONTROL) => {
            app.zoom = app.zoom.bump(1);
        }
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::CONTROL) => {
            app.zoom = app.zoom.bump(-1);
        }
        MouseEventKind::ScrollUp => {
            app.selected = app.selected.saturating_sub(1);
        }
        MouseEventKind::ScrollDown => {
            if !app.rows.is_empty() {
                app.selected = (app.selected + 1).min(app.rows.len() - 1);
            }
        }
        MouseEventKind::Down(_) if app.hits_area.contains(pos) => {
            let row = (m.row.saturating_sub(app.hits_area.y)) as usize;
            let i = app.list_start.saturating_add(row);
            if i < app.rows.len() {
                app.selected = i;
            }
        }
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if app.help && !matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
        app.help = false;
        return false;
    }
    match key.code {
        KeyCode::Char('c' | 'q') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Esc if app.help => app.help = false,
        KeyCode::Esc => return true,
        KeyCode::Char('?') => app.help = !app.help,
        KeyCode::Enter => app.open_selected(),
        KeyCode::F(3) => app.preview_selected(),
        KeyCode::F(4) => {
            app.surface = if app.surface == Surface::Tree {
                Surface::Auto
            } else {
                Surface::Tree
            };
        }
        KeyCode::F(6) => app.show_weight = !app.show_weight,
        KeyCode::Char('=') | KeyCode::Char('+')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.zoom = app.zoom.bump(1);
        }
        KeyCode::Char('-') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.zoom = app.zoom.bump(-1);
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reveal_selected();
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.copy_selected();
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.drag_selected();
        }
        KeyCode::F(2) => app.drag_selected(),
        KeyCode::Tab => app.drag_selected(),
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scope = if app.scope == Scope::Folders {
                Scope::All
            } else {
                Scope::Folders
            };
            app.mark_dirty();
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.sort = next_sort(app.sort);
            app.mark_dirty();
        }
        KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.match_mode = app.match_mode.cycle();
            app.status = format!("match  {}", app.match_mode.as_str());
            app.mark_dirty();
        }
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.zebra = !app.zebra;
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.class = next_class(app.class);
            app.mark_dirty();
        }
        KeyCode::Backspace => {
            app.query.pop();
            app.mark_dirty();
        }
        KeyCode::Down | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if !app.rows.is_empty() {
                app.selected = (app.selected + 1).min(app.rows.len() - 1);
            }
        }
        KeyCode::Down => {
            if !app.rows.is_empty() {
                app.selected = (app.selected + 1).min(app.rows.len() - 1);
            }
        }
        KeyCode::Up | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.selected = app.selected.saturating_sub(1);
        }
        KeyCode::Up => app.selected = app.selected.saturating_sub(1),
        KeyCode::PageDown => {
            app.selected = (app.selected + 20).min(app.rows.len().saturating_sub(1));
        }
        KeyCode::PageUp => app.selected = app.selected.saturating_sub(20),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.query.push(c);
            app.mark_dirty();
        }
        _ => {}
    }
    false
}

fn next_class(class: FileClass) -> FileClass {
    match class {
        FileClass::All => FileClass::Image,
        FileClass::Image => FileClass::Audio,
        FileClass::Audio => FileClass::Video,
        FileClass::Video => FileClass::Document,
        FileClass::Document => FileClass::Archive,
        FileClass::Archive => FileClass::All,
    }
}

fn pipe_copy(bin: &str, args: &[&str], text: &str) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    let Ok(mut child) = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn next_sort(sort: Sort) -> Sort {
    match sort {
        Sort::Score => Sort::Name,
        Sort::Name => Sort::NameDesc,
        Sort::NameDesc => Sort::Newest,
        Sort::Newest => Sort::Oldest,
        Sort::Oldest => Sort::Largest,
        Sort::Largest => Sort::Smallest,
        Sort::Smallest => Sort::Score,
    }
}

fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::Score => "score",
        Sort::Name => "name",
        Sort::NameDesc => "name Z–A",
        Sort::Newest => "newest",
        Sort::Oldest => "oldest",
        Sort::Largest => "largest",
        Sort::Smallest => "smallest",
    }
}

fn class_label(class: FileClass) -> &'static str {
    match class {
        FileClass::All => "all",
        FileClass::Image => "images",
        FileClass::Audio => "audio",
        FileClass::Video => "video",
        FileClass::Document => "docs",
        FileClass::Archive => "archives",
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let chunks = if app.show_weight {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(7),
                Constraint::Length(1),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(area)
    };

    draw_header(frame, chunks[0], app);
    draw_prompt(frame, chunks[1], app);
    match app.surface {
        Surface::Tree => draw_tree(frame, chunks[2], app),
        Surface::Auto if app.zoom.is_grid() => draw_grid(frame, chunks[2], app),
        Surface::Auto => draw_list(frame, chunks[2], app),
    }
    if app.show_weight {
        draw_weight(frame, chunks[3], app);
        draw_footer(frame, chunks[4], app);
    } else {
        draw_footer(frame, chunks[3], app);
    }

    if app.help {
        draw_help(frame, area);
    }
}

fn surface_label(app: &App) -> &'static str {
    if app.surface == Surface::Tree {
        "tree"
    } else if app.zoom.is_grid() {
        "grid"
    } else {
        "list"
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut chips = vec![
        Chip::new("Qfind", BG, ACCENT),
        Chip::new(
            format!("{} files", compact(app.catalog.file_count())),
            TEXT,
            SURFACE,
        ),
    ];
    if area.width >= 48 {
        chips.push(Chip::new(app.match_mode.as_str(), BG, PINK));
    }
    if area.width >= 60 {
        chips.push(Chip::new(sort_label(app.sort), BG, PURPLE));
    }
    if area.width >= 74 {
        chips.push(Chip::new(
            format!("{}% {}", app.zoom.get(), surface_label(app)),
            TEXT,
            SURFACE,
        ));
    }
    if area.width >= 90 {
        let scope = if app.scope == Scope::Folders {
            "folders"
        } else {
            "all"
        };
        chips.push(Chip::new(scope, BG, SKY));
    }
    if area.width >= 104 && app.class != FileClass::All {
        chips.push(Chip::new(class_label(app.class), TEXT, SURFACE));
    }
    let chips = fit_chips(chips, area.width);
    frame.render_widget(Paragraph::new(powerline(&chips, area.width, BG)), area);
}

fn draw_prompt(frame: &mut Frame, area: Rect, app: &App) {
    let line = Line::from(vec![
        Span::styled(
            format!(" {}  ", icon_prompt()),
            Style::new()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.query.clone(), Style::new().fg(TEXT).bg(SURFACE)),
        Span::styled(
            "█",
            Style::new()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::new().bg(SURFACE)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    frame.render_widget(Block::default().style(Style::new().bg(BG)), area);
    app.hits_area = area;
    let height = area.height as usize;
    let mut start = 0usize;
    if app.selected >= height {
        start = app.selected.saturating_sub(height.saturating_sub(1));
    }
    app.list_start = start;
    let end = (start + height).min(app.rows.len());
    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in app.rows.get(start..end).into_iter().flatten().enumerate() {
        let idx = start + i;
        let selected = idx == app.selected;
        let zebra = app.zebra && idx % 2 == 1;
        lines.push(row_line(row, selected, zebra, area.width));
    }
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(BG)), area);

    if app.rows.len() > height {
        let mut state =
            ScrollbarState::new(app.rows.len().saturating_sub(1)).position(app.selected);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(DIM))
                .thumb_style(Style::new().fg(ACCENT)),
            area,
            &mut state,
        );
    }
}

fn draw_grid(frame: &mut Frame, area: Rect, app: &mut App) {
    frame.render_widget(Block::default().style(Style::new().bg(BG)), area);
    let cell_w = (12 + u16::from(app.zoom.get()) / 5).max(10);
    let cell_h = (2 + u16::from(app.zoom.get().saturating_sub(40)) / 20).max(2);
    app.hits_area = area;
    let cols = (area.width / cell_w).max(1) as usize;
    let rows_n = (area.height / cell_h).max(1) as usize;
    let start = app.selected.saturating_sub(app.selected % cols.max(1));
    app.list_start = start;
    for (i, row) in app.rows.iter().enumerate().skip(start).take(cols * rows_n) {
        let local = i - start;
        let c = (local % cols) as u16;
        let r = (local / cols) as u16;
        let x = area.x + c * cell_w;
        let y = area.y + r * cell_h;
        if y >= area.y + area.height {
            break;
        }
        let w = cell_w.min(area.x + area.width - x);
        let h = cell_h.min(area.y + area.height - y);
        let selected = i == app.selected;
        let icon = if row.is_dir {
            icon_folder()
        } else {
            icon_file()
        };
        let (fg, bg) = if selected {
            (ACCENT, SELECT_BG)
        } else if row.is_dir {
            (ACCENT, hsl_tile(&row.name, i, 0.58, 0.18))
        } else {
            (TEXT, hsl_tile(&row.name, i, 0.52, 0.14))
        };
        let label = if selected {
            format!("{} {icon} {}", theme::BAR, row.name)
        } else {
            format!("  {icon} {}", row.name)
        };
        frame.render_widget(
            Paragraph::new(label).style(Style::new().fg(fg).bg(bg).add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            })),
            Rect::new(x, y, w, h),
        );
    }
}

fn draw_tree(frame: &mut Frame, area: Rect, app: &mut App) {
    use qfind_core::{fold_stems, walk_visible};
    let items: Vec<HitRef> = app
        .rows
        .iter()
        .map(|r| HitRef {
            id: None,
            path: r.path.to_string_lossy().into_owned(),
            is_dir: r.is_dir,
            weight: 1,
        })
        .collect();
    let stems = fold_stems(&items);
    let flat = walk_visible(&stems, &|_| true);
    frame.render_widget(Block::default().style(Style::new().bg(BG)), area);
    app.hits_area = area;
    app.list_start = 0;
    let lines: Vec<Line> = flat
        .into_iter()
        .take(area.height as usize)
        .map(|f| {
            let pad = "  ".repeat(f.depth as usize);
            let mark = if f.stem.is_dir {
                icon_folder()
            } else {
                icon_file()
            };
            let label = if f.stem.is_dir {
                f.stem.name.clone()
            } else {
                let (stem, ext) = qfind_core::split_filename(&f.stem.name, false);
                if ext.is_empty() {
                    stem.to_string()
                } else {
                    format!("{stem}.{ext}")
                }
            };
            Line::from(Span::styled(
                format!("{pad}{mark} {label}"),
                if f.stem.is_dir {
                    Style::new().fg(ACCENT).bg(BG)
                } else {
                    Style::new().fg(TEXT).bg(BG)
                },
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(BG)), area);
}

fn draw_weight(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<HitRef> = app
        .rows
        .iter()
        .map(|r| HitRef {
            id: None,
            path: r.path.to_string_lossy().into_owned(),
            is_dir: r.is_dir,
            weight: 1,
        })
        .collect();
    let folders = folder_weights(&items);
    let inner = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .inner(area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(DIM))
            .style(Style::new().bg(BG))
            .title(chip("WeightMap", BG, ACCENT)),
        area,
    );
    if folders.is_empty() {
        return;
    }
    let tiles = squarify(folders, f64::from(inner.width), f64::from(inner.height));
    for (i, t) in tiles.into_iter().enumerate() {
        let x = inner.x.saturating_add(t.x as u16);
        let y = inner.y.saturating_add(t.y as u16);
        let w = (t.w as u16).max(1);
        let h = (t.h as u16).max(1);
        if x >= inner.x + inner.width || y >= inner.y + inner.height {
            continue;
        }
        let w = w.min(inner.x + inner.width - x);
        let h = h.min(inner.y + inner.height - y);
        let bg = hsl_tile(&t.name, i, 0.72, 0.30);
        frame.render_widget(
            Paragraph::new(t.name).style(Style::new().bg(bg).fg(TEXT).add_modifier(Modifier::BOLD)),
            Rect::new(x, y, w, h),
        );
    }
}

fn row_line(row: &Row, selected: bool, zebra: bool, width: u16) -> Line<'static> {
    let bg = if selected {
        SELECT_BG
    } else if zebra {
        ZEBRA
    } else {
        BG
    };
    let bar = if selected {
        Span::styled(
            format!("{} ", theme::BAR),
            Style::new().fg(ACCENT).bg(bg).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  ", Style::new().bg(bg))
    };
    let glyph = if row.is_dir {
        icon_folder()
    } else {
        icon_file()
    };
    let icon = Span::styled(
        format!("{glyph} "),
        Style::new()
            .fg(if row.is_dir { ACCENT } else { DIM })
            .bg(bg),
    );
    let mut spans = vec![bar, icon];
    spans.extend(highlight_chars(&row.name, &row.indices, bg));
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let path = row.path.display().to_string();
    let remain = (width as usize).saturating_sub(used + 1);
    let shown = if remain == 0 {
        String::new()
    } else if path.chars().count() + 1 > remain {
        let take = remain.saturating_sub(2);
        let tail: String = path.chars().rev().take(take).collect::<String>();
        format!(" …{}", tail.chars().rev().collect::<String>())
    } else {
        format!(" {path}")
    };
    let shown_cols = shown.chars().count();
    spans.push(Span::styled(shown, Style::new().fg(DIM).bg(bg)));
    let pad = (width as usize).saturating_sub(used + shown_cols);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Style::new().bg(bg)));
    }
    Line::from(spans)
}

fn highlight_chars(name: &str, indices: &[u32], bg: ratatui::style::Color) -> Vec<Span<'static>> {
    let set: HashSet<u32> = indices.iter().copied().collect();
    name.chars()
        .enumerate()
        .map(|(i, ch)| {
            let matched = set.contains(&(i as u32));
            let style = if matched {
                Style::new().bg(bg).fg(MATCH).add_modifier(Modifier::BOLD)
            } else {
                Style::new().bg(bg).fg(TEXT)
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let status = format!(" {} ", app.status);
    let sw = (status.chars().count() as u16)
        .min(area.width.saturating_sub(8))
        .max(1);
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(8), Constraint::Length(sw)])
        .split(area);
    let chips = if app.dragging {
        vec![Chip::new("dragging… drop on a window", BG, MATCH)]
    } else {
        vec![
            Chip::new("↑↓ nav", TEXT, SURFACE),
            Chip::new("⏎ open", BG, ACCENT),
            Chip::new("F3 preview", TEXT, SURFACE),
            Chip::new("F4 tree", BG, SKY),
            Chip::new("F6 weight", TEXT, SURFACE),
            Chip::new("? help", BG, PURPLE),
        ]
    };
    let chips = fit_chips(chips, split[0].width);
    frame.render_widget(
        Paragraph::new(powerline(&chips, split[0].width, BG)),
        split[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(status, Style::new().fg(DIM).bg(BG)))
            .alignment(Alignment::Right)
            .style(Style::new().bg(BG)),
        split[1],
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let w = 62.min(area.width.saturating_sub(6));
    let h = 18.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);
    frame.render_widget(Clear, popup);
    let dim = Style::new().fg(DIM).bg(SURFACE);
    let txt = Style::new().fg(TEXT).bg(SURFACE);
    let text = vec![
        Line::from(vec![
            chip("keys", BG, ACCENT),
            Span::styled("  same bindings  ", dim),
        ]),
        Line::from(""),
        Line::from(vec![
            chip("type", BG, ACCENT),
            Span::styled(" query   ", txt),
            chip("^m", BG, PINK),
            Span::styled(" fuzzy / substring / exact", txt),
        ]),
        Line::from(vec![
            chip("*.wav", TEXT, SURFACE),
            Span::styled(" glob token", dim),
        ]),
        Line::from(vec![
            chip("tab", BG, MATCH),
            chip("F2", BG, MATCH),
            chip("^d", BG, MATCH),
            Span::styled(" drag selected", txt),
        ]),
        Line::from(vec![
            chip("^f", BG, SKY),
            Span::styled(" folders only   ", txt),
            chip("^s", BG, PURPLE),
            Span::styled(" sort   ", txt),
            chip("^t", TEXT, SURFACE),
            Span::styled(" type", txt),
        ]),
        Line::from(vec![
            chip("F3", TEXT, SURFACE),
            Span::styled(" preview   ", txt),
            chip("^o", TEXT, SURFACE),
            Span::styled(" files   ", txt),
            chip("^y", TEXT, SURFACE),
            Span::styled(" copy", txt),
        ]),
        Line::from(vec![
            chip("F4", BG, SKY),
            Span::styled(" tree   ", txt),
            chip("F6", TEXT, SURFACE),
            Span::styled(" weight   ", txt),
            chip("^z", TEXT, SURFACE),
            Span::styled(" zebra", txt),
        ]),
        Line::from(vec![
            chip("⏎", BG, ACCENT),
            Span::styled(" open   ", txt),
            chip("esc", TEXT, SURFACE),
            Span::styled(" quit", txt),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::new().fg(TEXT).bg(SURFACE))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(ACCENT))
                    .style(Style::new().bg(SURFACE))
                    .title(chip("help", BG, ACCENT)),
            ),
        popup,
    );
}
