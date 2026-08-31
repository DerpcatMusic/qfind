use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use qfind_core::{
    Catalog, Config, DateAge, FileClass, Hit, HitRef, MatchMode, OpenHow, OpenMode, Scope,
    SearchOpts, Sort, Surface, Weighted, Zoom, default_snapshot_path, folder_weights, squarify,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::picker::Picker;
use ratatui_image::{Image, Resize, StatefulImage};

mod dnd;
mod overlay;
mod preview;
mod query;
mod reactor;
mod splash;
mod surface;
mod theme;
use theme::{Chip, Theme, compact, fit_chips, icon_for, icon_prompt, toolbar};

const MAX_ROWS: usize = 2_000;

/// Open the Qfind TUI. Rebuilds the Catalog on first launch if missing.
pub fn run() -> Result<()> {
    let snapshot = default_snapshot_path();
    let cfg = Config::load();
    let theme = Theme::parse(&cfg.theme);
    crossterm::style::force_color_output(true);
    let mut terminal = ratatui::init();
    let picker = graphics_picker();
    let _ = enable_mouse();
    let result = (|| -> Result<Option<(String, Vec<String>, PathBuf)>> {
        let catalog = if snapshot.exists() {
            Catalog::open(&snapshot).with_context(|| format!("open {}", snapshot.display()))?
        } else {
            splash::rebuild_catalog(&mut terminal, &theme, cfg.rebuild_to(&snapshot))?
        };
        let warm = catalog.clone();
        thread::spawn(move || warm.warm());
        let (mut reactor, events) = reactor::Reactor::new();
        let mut app = App::new(catalog, theme, picker, events);
        app.kick_search();
        event_loop(&mut terminal, &mut app, &mut reactor)?;
        Ok(app.exit_command.take())
    })();
    let _ = disable_mouse();
    ratatui::restore();
    match result? {
        Some((program, args, path)) => launch_editor(&program, &args, &path),
        None => Ok(()),
    }
}

fn graphics_picker() -> Picker {
    let hinted = std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("WEZTERM_EXECUTABLE").is_some()
        || std::env::var_os("ITERM_SESSION_ID").is_some();
    if hinted {
        Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
    } else {
        Picker::halfblocks()
    }
}

fn enable_mouse() -> io::Result<()> {
    let mut out = io::stdout();
    execute!(out, EnableMouseCapture)?;
    // Any-event tracking: hover without a button (Grok CLI does this).
    write!(out, "\x1b[?1003h")?;
    out.flush()?;
    dnd::enable()
}

fn disable_mouse() -> io::Result<()> {
    let _ = dnd::disable();
    let mut out = io::stdout();
    write!(out, "\x1b[?1003l")?;
    execute!(out, DisableMouseCapture)?;
    out.flush()
}

#[derive(Clone)]
struct Row {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    indices: Vec<u32>,
}

impl Row {
    fn from_hit(hit: Hit<'_>) -> Self {
        Self {
            name: hit.name().to_string(),
            path: hit.path(),
            is_dir: hit.is_dir(),
            size: hit.size(),
            indices: hit.indices().to_vec(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    Search,
    Results,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BrowserPane {
    Folders,
    #[default]
    Items,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WeightMode {
    #[default]
    Size,
    Format,
}

impl WeightMode {
    fn cycle(self) -> Self {
        match self {
            Self::Size => Self::Format,
            Self::Format => Self::Size,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Format => "file types",
        }
    }
}

#[derive(Clone)]
struct WeightHit {
    area: Rect,
    path: String,
    is_dir: bool,
}

enum WorkEvent {
    Query(query::Event),
    Preview(Box<preview::Event>),
    Catalog(std::result::Result<PathBuf, String>),
}

struct App {
    catalog: Catalog,
    query: String,
    rows: Vec<Row>,
    selected: usize,
    status: String,
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
    query_session: query::Session,
    searching: bool,
    theme: Theme,
    open: OpenMode,
    editor: String,
    hover: Option<usize>,
    last_click: Option<(Instant, u16, u16)>,
    scroll: usize,
    view_h: usize,
    scroll_bar: Rect,
    dragging_bar: bool,
    overlays: overlay::Stack,
    browser_root: Option<PathBuf>,
    browser_dir: Option<PathBuf>,
    browser_folders: Vec<Row>,
    browser_items: Vec<Row>,
    browser_pane: BrowserPane,
    folder_selected: usize,
    folder_scroll: usize,
    item_selected: usize,
    item_scroll: usize,
    folders_area: Rect,
    items_area: Rect,
    folders_bar: Rect,
    items_bar: Rect,
    weight_mode: WeightMode,
    weight_area: Rect,
    weight_mode_area: Rect,
    weight_hits: Vec<WeightHit>,
    weight_hover: Option<usize>,
    browser_hover: Option<(BrowserPane, usize)>,
    scroll_hover: bool,
    header_hot: Vec<(u16, u16, ChipHit)>,
    header_area: Rect,
    footer_area: Rect,
    frame_area: Rect,
    prompt_area: Rect,
    content_area: Rect,
    preview_area: Rect,
    preview_divider: Rect,
    weight_panel_area: Rect,
    folder_pane: Rect,
    item_pane: Rect,
    previews: preview::Pipeline,
    preview_width: u8,
    preview_scroll: u16,
    preview_max_scroll: u16,
    dragging_preview: bool,
    preview_hover: bool,
    focus: Focus,
    grid_cols: usize,
    grid_cell_w: u16,
    grid_cell_h: u16,
    show_hidden: bool,
    respect_gitignore: bool,
    respect_ignore: bool,
    excluded_paths: Vec<PathBuf>,
    events: reactor::Sender<WorkEvent>,
    rebuilding: bool,
    rebuild_pending: bool,
    exit_command: Option<(String, Vec<String>, PathBuf)>,
}

#[derive(Clone, Copy)]
enum ChipHit {
    Brand,
    Match,
    Sort,
    Surface,
    Scope,
    Skin,
}

impl App {
    fn new(
        catalog: Catalog,
        theme: Theme,
        image_picker: Picker,
        events: reactor::Sender<WorkEvent>,
    ) -> Self {
        let cfg = Config::load();
        let query_session = query::Session::new(
            catalog.clone(),
            events.clone(),
            cfg.respect_gitignore,
            cfg.respect_ignore,
        );
        let previews = preview::Pipeline::new(image_picker, events.clone());
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
            query_session,
            searching: false,
            theme,
            open: cfg.open,
            editor: cfg.editor,
            hover: None,
            last_click: None,
            scroll: 0,
            view_h: 1,
            scroll_bar: Rect::default(),
            dragging_bar: false,
            overlays: overlay::Stack::default(),
            browser_root: None,
            browser_dir: None,
            browser_folders: Vec::new(),
            browser_items: Vec::new(),
            browser_pane: BrowserPane::Items,
            folder_selected: 0,
            folder_scroll: 0,
            item_selected: 0,
            item_scroll: 0,
            folders_area: Rect::default(),
            items_area: Rect::default(),
            folders_bar: Rect::default(),
            items_bar: Rect::default(),
            weight_mode: WeightMode::Size,
            weight_area: Rect::default(),
            weight_mode_area: Rect::default(),
            weight_hits: Vec::new(),
            weight_hover: None,
            browser_hover: None,
            scroll_hover: false,
            header_hot: Vec::new(),
            header_area: Rect::default(),
            footer_area: Rect::default(),
            frame_area: Rect::default(),
            prompt_area: Rect::default(),
            content_area: Rect::default(),
            preview_area: Rect::default(),
            preview_divider: Rect::default(),
            weight_panel_area: Rect::default(),
            folder_pane: Rect::default(),
            item_pane: Rect::default(),
            previews,
            preview_width: cfg.preview_width,
            preview_scroll: 0,
            preview_max_scroll: 0,
            dragging_preview: false,
            preview_hover: false,
            focus: Focus::Search,
            grid_cols: 1,
            grid_cell_w: 1,
            grid_cell_h: 1,
            show_hidden: cfg.show_hidden,
            respect_gitignore: cfg.respect_gitignore,
            respect_ignore: cfg.respect_ignore,
            excluded_paths: cfg.exclude_paths,
            events,
            rebuilding: false,
            rebuild_pending: false,
            exit_command: None,
        }
    }

    fn refresh_preview(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            self.preview_scroll = 0;
            self.previews.clear_side();
            return;
        };
        if self.previews.side.path.as_deref() != Some(row.path.as_path()) {
            self.preview_scroll = 0;
        }
        self.previews.select(&row.path, row.is_dir);
    }

    fn measure_preview(&mut self) {
        let body_height = self.preview_area.height.saturating_sub(4) as usize;
        let width = self.preview_area.width.saturating_sub(2);
        let lines = if width == 0 || body_height == 0 {
            0
        } else {
            side_preview_lines(self)
                .iter()
                .map(|line| line.width().div_ceil(width as usize).max(1))
                .sum()
        };
        self.preview_max_scroll = lines.saturating_sub(body_height).min(u16::MAX as usize) as u16;
        self.preview_scroll = self.preview_scroll.min(self.preview_max_scroll);
    }

    fn schedule_visible_thumbnails(&mut self) {
        if self.surface == Surface::Tree || !self.zoom.is_grid() {
            return;
        }
        if self.searching {
            return;
        }
        let area = self.hits_area;
        let cols = self.grid_cols.max(1);
        let cell_w = self.grid_cell_w.max(1);
        let cell_h = self.grid_cell_h.max(1);
        let end = (self.list_start + self.view_h).min(self.rows.len());
        let mut viewport = Vec::with_capacity(end.saturating_sub(self.list_start));
        for i in self.list_start..end {
            let row = &self.rows[i];
            if row.is_dir {
                continue;
            }
            let local = i - self.list_start;
            let x = area.x + (local % cols) as u16 * cell_w;
            let y = area.y + (local / cols) as u16 * cell_h;
            if x >= area.right() || y >= area.bottom() {
                continue;
            }
            let width = cell_w.min(area.right() - x).saturating_sub(3).max(1);
            let height = cell_h.min(area.bottom() - y).saturating_sub(3).max(1);
            let path = row.path.clone();
            viewport.push((path, Size::new(width, height)));
        }
        self.previews.request_viewport(viewport);
    }

    fn invalidate_thumbnail_jobs(&mut self) {
        self.previews.invalidate_grid();
    }

    fn ensure_visible(&mut self) {
        surface::ensure_visible(
            self.selected,
            &mut self.scroll,
            self.rows.len(),
            self.view_h,
        );
    }

    fn selected_row(&self) -> Option<&Row> {
        if self.surface == Surface::Tree {
            match self.browser_pane {
                BrowserPane::Folders => self.browser_folders.get(self.folder_selected),
                BrowserPane::Items => self.browser_items.get(self.item_selected),
            }
        } else {
            self.rows.get(self.selected)
        }
    }

    fn browse(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if !path.is_dir() {
            return;
        }
        let items = read_directory(&path, self.show_hidden);
        let mut folders = vec![Row {
            name: ".  current".into(),
            path: path.clone(),
            is_dir: true,
            size: 0,
            indices: Vec::new(),
        }];
        if let Some(parent) = path.parent() {
            folders.push(Row {
                name: "..  parent".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                size: 0,
                indices: Vec::new(),
            });
        }
        folders.extend(items.iter().filter(|row| row.is_dir).cloned());
        self.browser_root = Some(path.clone());
        self.browser_dir = Some(path);
        self.browser_folders = folders;
        self.browser_items = items;
        self.browser_pane = BrowserPane::Items;
        self.folder_selected = 0;
        self.folder_scroll = 0;
        self.item_selected = 0;
        self.item_scroll = 0;
        self.weight_hover = None;
        self.browser_hover = None;
        self.surface = Surface::Tree;
        self.focus = Focus::Results;
    }

    fn preview_folder(&mut self) {
        let Some(path) = self
            .browser_folders
            .get(self.folder_selected)
            .map(|row| row.path.clone())
        else {
            return;
        };
        self.browser_dir = Some(path.clone());
        self.browser_items = read_directory(&path, self.show_hidden);
        self.item_selected = 0;
        self.item_scroll = 0;
        self.weight_hover = None;
        self.browser_hover = None;
    }

    fn ensure_browser_visible(&mut self) {
        let (selected, scroll, height, total) = match self.browser_pane {
            BrowserPane::Folders => (
                self.folder_selected,
                &mut self.folder_scroll,
                self.folders_area.height as usize,
                self.browser_folders.len(),
            ),
            BrowserPane::Items => (
                self.item_selected,
                &mut self.item_scroll,
                self.items_area.height as usize,
                self.browser_items.len(),
            ),
        };
        surface::ensure_visible(selected, scroll, total, height);
    }

    fn enter_selected(&mut self) {
        let Some((path, is_dir)) = self
            .selected_row()
            .map(|row| (row.path.clone(), row.is_dir))
        else {
            return;
        };
        if is_dir {
            self.browse(path);
        } else {
            self.open_selected();
        }
    }

    fn browse_from_selection(&mut self) {
        let path = self
            .rows
            .get(self.selected)
            .map(|row| {
                if row.is_dir {
                    row.path.clone()
                } else {
                    row.path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf()
                }
            })
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        self.browse(path);
    }

    fn cycle_open(&mut self) {
        self.open = self.open.cycle();
        let mut cfg = Config::load();
        cfg.open = self.open;
        let _ = cfg.save();
        self.status = format!("open  {}", self.open.as_str());
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
        self.invalidate_thumbnail_jobs();
        self.previews.clear_side();
        self.kick_search();
    }

    fn kick_search(&mut self) {
        self.query_session
            .submit(self.query.clone(), self.opts(), self.show_hidden);
        self.searching = true;
        self.status = "searching…".into();
    }

    fn apply_query(&mut self, message: query::Event) {
        match message {
            query::Event::Hits(generation, Ok(rows))
                if self.query_session.is_current(generation) =>
            {
                self.searching = false;
                self.selected = 0;
                self.scroll = 0;
                self.hover = None;
                self.status = format!(
                    "{} hits  ·  {} folders · {} files",
                    rows.len(),
                    self.catalog.folder_count(),
                    self.catalog.file_count()
                );
                self.rows = rows;
            }
            query::Event::Hits(generation, Err(err))
                if self.query_session.is_current(generation) =>
            {
                self.searching = false;
                self.status = err;
                self.rows.clear();
            }
            query::Event::Sizes(generation, rows) if self.query_session.is_current(generation) => {
                self.searching = false;
                self.rows = rows;
            }
            _ => {}
        }
    }

    fn apply_work(&mut self, event: WorkEvent) -> bool {
        match event {
            WorkEvent::Query(message) => self.apply_query(message),
            WorkEvent::Preview(event) => return self.previews.apply(*event),
            WorkEvent::Catalog(result) => self.finish_catalog_refresh(result),
        }
        true
    }

    fn request_catalog_refresh(&mut self) {
        if self.rebuilding {
            self.rebuild_pending = true;
            self.status = "Catalog refresh queued…".into();
            return;
        }
        self.rebuilding = true;
        self.status = "Refreshing Catalog in background…".into();
        let cfg = Config::load();
        let events = self.events.clone();
        let staging = catalog_staging_path();
        thread::spawn(move || {
            let result = Catalog::rebuild(cfg.rebuild_to(&staging))
                .map(|_| staging)
                .map_err(|error| error.to_string());
            let _ = events.send(WorkEvent::Catalog(result));
        });
    }

    fn finish_catalog_refresh(&mut self, result: std::result::Result<PathBuf, String>) {
        self.rebuilding = false;
        if self.rebuild_pending {
            self.rebuild_pending = false;
            self.request_catalog_refresh();
            return;
        }
        let catalog = result.and_then(publish_catalog);
        match catalog {
            Ok(catalog) => {
                self.catalog = catalog.clone();
                self.query_session = query::Session::new(
                    catalog,
                    self.events.clone(),
                    self.respect_gitignore,
                    self.respect_ignore,
                );
                self.kick_search();
            }
            Err(error) => self.status = format!("Catalog refresh failed: {error}"),
        }
    }

    fn reload_browser(&mut self) {
        let Some(root) = self.browser_root.clone() else {
            return;
        };
        let dir = self.browser_dir.clone();
        let folder = self
            .browser_folders
            .get(self.folder_selected)
            .map(|row| row.path.clone());
        let item = self
            .browser_items
            .get(self.item_selected)
            .map(|row| row.path.clone());
        self.browse(root);
        if let Some(dir) = dir {
            self.browser_dir = Some(dir.clone());
            self.browser_items = read_directory(&dir, self.show_hidden);
        }
        if let Some(folder) = folder
            && let Some(index) = self
                .browser_folders
                .iter()
                .position(|row| row.path == folder)
        {
            self.folder_selected = index;
        }
        if let Some(item) = item
            && let Some(index) = self.browser_items.iter().position(|row| row.path == item)
        {
            self.item_selected = index;
        }
        self.ensure_browser_visible();
    }

    fn open_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let path = row.path.clone();
        let is_dir = row.is_dir;
        let mut cfg = Config::load();
        cfg.open = self.open;
        cfg.editor.clone_from(&self.editor);
        match cfg.open_how(&path, is_dir) {
            OpenHow::Desktop => self.spawn_desktop(&path),
            OpenHow::Editor { program, args } => {
                self.exit_command = Some((program, args, path));
            }
        }
    }

    fn spawn_desktop(&mut self, path: &std::path::Path) {
        let (name, result) = desktop_open(path);
        match result {
            Ok(_) => self.status = format!("{name}  {}", path.display()),
            Err(err) => self.status = format!("{name}: {err}"),
        }
    }

    fn preview_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let path = row.path.clone();
        #[cfg(target_os = "linux")]
        {
            let mut command = Command::new("sushi");
            command.arg(&path);
            if spawn_detached(&mut command).is_ok() {
                self.status = format!("preview  {}", path.display());
                return;
            }
        }
        #[cfg(target_os = "macos")]
        {
            let mut command = Command::new("qlmanage");
            command.arg("-p").arg(&path);
            if spawn_detached(&mut command).is_ok() {
                self.status = format!("Quick Look  {}", path.display());
                return;
            }
        }
        self.spawn_desktop(&path);
    }

    fn reveal_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let path = row.path.clone();
        #[cfg(target_os = "linux")]
        {
            let uri = format!("file://{}", path.display());
            let mut command = Command::new("gdbus");
            command.args([
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
            ]);
            let dbus = spawn_detached(&mut command);
            if dbus.is_ok() {
                self.status = format!("show in files  {}", path.display());
                return;
            }
        }
        #[cfg(target_os = "macos")]
        {
            let mut command = Command::new("open");
            command.arg("-R").arg(&path);
            match spawn_detached(&mut command) {
                Ok(_) => self.status = format!("show in Finder  {}", path.display()),
                Err(err) => self.status = format!("reveal: {err}"),
            }
            return;
        }
        #[cfg(target_os = "windows")]
        {
            let mut command = Command::new("explorer.exe");
            command.arg("/select,").arg(&path);
            match spawn_detached(&mut command) {
                Ok(_) => self.status = format!("show in Explorer  {}", path.display()),
                Err(err) => self.status = format!("reveal: {err}"),
            }
            return;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let folder = if row.is_dir {
                path.clone()
            } else {
                path.parent().unwrap_or(path.as_path()).to_path_buf()
            };
            let (name, result) = desktop_open(&folder);
            match result {
                Ok(_) => self.status = format!("{name}  {}", folder.display()),
                Err(err) => self.status = format!("reveal: {err}"),
            }
        }
    }

    fn copy_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let text = row.path.to_string_lossy().into_owned();
        #[cfg(target_os = "linux")]
        let (ok, hint) = (
            pipe_copy("wl-copy", &[], &text)
                || pipe_copy("xclip", &["-selection", "clipboard"], &text),
            "copy: install wl-copy or xclip",
        );
        #[cfg(target_os = "macos")]
        let (ok, hint) = (pipe_copy("pbcopy", &[], &text), "copy: pbcopy unavailable");
        #[cfg(target_os = "windows")]
        let (ok, hint) = (
            pipe_copy("clip.exe", &[], &text),
            "copy: clip.exe unavailable",
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let (ok, hint) = (false, "copy unavailable");
        self.status = if ok {
            format!("copied  {text}")
        } else {
            hint.into()
        };
    }
}

fn catalog_staging_path() -> PathBuf {
    let path = default_snapshot_path();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("catalog");
    path.with_file_name(format!("{name}-refresh"))
}

fn publish_catalog(staging: PathBuf) -> std::result::Result<Catalog, String> {
    let snapshot = default_snapshot_path();
    let staging_mask = staging.with_extension("mask");
    let snapshot_mask = snapshot.with_extension("mask");
    std::fs::rename(&staging, &snapshot).map_err(|error| error.to_string())?;
    if !staging_mask.exists() || std::fs::rename(&staging_mask, &snapshot_mask).is_err() {
        let _ = std::fs::remove_file(snapshot_mask);
    }
    Catalog::open(snapshot).map_err(|error| error.to_string())
}

fn read_directory(path: &Path, show_hidden: bool) -> Vec<Row> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            show_hidden
                || entry
                    .file_name()
                    .as_encoded_bytes()
                    .first()
                    .is_none_or(|byte| *byte != b'.')
        })
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some(Row {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                is_dir: meta.is_dir(),
                size: if meta.is_file() { meta.len() } else { 0 },
                indices: Vec::new(),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    reactor: &mut reactor::Reactor<WorkEvent>,
) -> Result<()> {
    let mut dirty = true;
    loop {
        if dirty {
            terminal.autoresize()?;
            let size = terminal.size()?;
            surface::prepare(app, Rect::new(0, 0, size.width, size.height));
            if !app.searching {
                app.refresh_preview();
            }
            app.measure_preview();
            terminal.draw(|f| draw(f, app))?;
            app.schedule_visible_thumbnails();
            dirty = false;
        }
        match reactor.wait()? {
            reactor::Event::Work(event) => dirty |= app.apply_work(event),
            reactor::Event::Terminal(Event::Osc72 { kind, x, y }) => {
                if let Some(event) = dnd::decode(kind, x, y) {
                    handle_dnd(app, event);
                    dirty = true;
                }
            }
            reactor::Event::Terminal(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if handle_key(app, key) {
                    break;
                }
                dirty = true;
            }
            reactor::Event::Terminal(Event::Mouse(m)) => {
                let hover_before = app.hover;
                let weight_before = app.weight_hover;
                let browser_before = app.browser_hover;
                let scroll_before = app.scroll_hover;
                let preview_before = app.preview_hover;
                let overlay_before = format!("{:?}", app.overlays);
                handle_mouse(app, m);
                if app.hover != hover_before
                    || app.weight_hover != weight_before
                    || app.browser_hover != browser_before
                    || app.scroll_hover != scroll_before
                    || app.preview_hover != preview_before
                    || overlay_before != format!("{:?}", app.overlays)
                    || !matches!(m.kind, MouseEventKind::Moved)
                {
                    dirty = true;
                }
            }
            reactor::Event::Terminal(Event::Resize(_, _)) => dirty = true,
            reactor::Event::Terminal(_) => {}
        }
        if app.exit_command.is_some() {
            break;
        }
    }
    Ok(())
}

fn handle_dnd(app: &mut App, event: dnd::Event) {
    match event {
        dnd::Event::Offer { x, y } => {
            let pos = Position::new(x, y);
            let row = if app.dragging_preview || app.dragging_bar {
                None
            } else if app.surface == Surface::Tree && app.folders_area.contains(pos) {
                app.browser_folders
                    .get(app.folder_scroll + y.saturating_sub(app.folders_area.y) as usize)
            } else if app.surface == Surface::Tree && app.items_area.contains(pos) {
                app.browser_items
                    .get(app.item_scroll + y.saturating_sub(app.items_area.y) as usize)
            } else if app.hits_area.contains(pos) {
                result_at(app, pos).and_then(|index| app.rows.get(index))
            } else {
                None
            };
            let Some((path, is_dir)) = row.map(|row| (row.path.clone(), row.is_dir)) else {
                let _ = dnd::reject();
                return;
            };
            let label = format!(
                "{}  {}",
                icon_for(&path, is_dir),
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            );
            let icon = app
                .previews
                .side
                .path
                .as_deref()
                .filter(|selected| *selected == path)
                .and(app.previews.side.drag_icon.as_ref())
                .map(|icon| (icon.data.as_slice(), icon.width, icon.height));
            match dnd::offer(&path, &label, icon) {
                Ok(()) => app.status = format!("dragging  {}", path.display()),
                Err(error) => app.status = format!("drag: {error}"),
            }
        }
        dnd::Event::End { canceled } => {
            app.status = if canceled {
                "drag canceled".into()
            } else {
                "file dropped".into()
            };
        }
        dnd::Event::Error => app.status = "drag rejected by terminal".into(),
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    let pos = Position::new(m.column, m.row);
    if !app.overlays.is_empty() {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let original = match app.overlays.top() {
                    Some(overlay::Layer::Theme { original, .. }) => Some(*original),
                    _ => None,
                };
                match overlay::click_top(&mut app.overlays, m.column, m.row, app.frame_area) {
                    overlay::Click::Theme(i) => {
                        if let Some(skin) = Theme::ALL.get(i).copied() {
                            app.theme = skin;
                            save_theme(app, skin);
                            app.overlays.pop();
                        }
                    }
                    overlay::Click::Menu(i) => {
                        app.overlays.pop();
                        match i {
                            0 => app.open_selected(),
                            1 => app.preview_selected(),
                            2 => app.copy_selected(),
                            3 => app.reveal_selected(),
                            _ => {}
                        }
                    }
                    overlay::Click::Settings(i) => {
                        if let Some(overlay::Layer::Settings { selected }) = app.overlays.top_mut()
                        {
                            *selected = i;
                        }
                        adjust_setting(app, i, 1, true);
                    }
                    overlay::Click::Closed => {
                        if let Some(original) = original {
                            app.theme = original;
                        }
                    }
                    overlay::Click::Miss => {}
                    overlay::Click::Ignore => {}
                }
            }
            MouseEventKind::Moved => {
                let loc = match app.overlays.top() {
                    Some(overlay::Layer::Menu { col, row, .. }) => Some((*col, *row)),
                    _ => None,
                };
                if let Some((col, row)) = loc {
                    let r = overlay::menu_rect(col, row, app.frame_area);
                    if r.contains(pos) {
                        let i = m.row.saturating_sub(r.y.saturating_add(1)) as usize;
                        if i < overlay::MENU_ITEMS.len()
                            && let Some(overlay::Layer::Menu { pick, .. }) = app.overlays.top_mut()
                        {
                            *pick = i;
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let selected = match app.overlays.top_mut() {
                    Some(overlay::Layer::Theme { selected, .. }) => {
                        if matches!(m.kind, MouseEventKind::ScrollUp) {
                            *selected = selected.saturating_sub(1);
                        } else {
                            *selected = (*selected + 1).min(Theme::ALL.len().saturating_sub(1));
                        }
                        Some(*selected)
                    }
                    Some(overlay::Layer::Settings { selected }) => {
                        if matches!(m.kind, MouseEventKind::ScrollUp) {
                            *selected = selected.saturating_sub(1);
                        } else {
                            *selected = (*selected + 1).min(overlay::SETTINGS_ITEMS - 1);
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(selected) = selected {
                    app.theme = Theme::ALL[selected];
                }
            }
            _ => {}
        }
        return;
    }

    match m.kind {
        MouseEventKind::ScrollUp if app.preview_area.contains(pos) => {
            app.preview_scroll = app.preview_scroll.saturating_sub(3);
        }
        MouseEventKind::ScrollDown if app.preview_area.contains(pos) => {
            app.preview_scroll = (app.preview_scroll + 3).min(app.preview_max_scroll);
        }
        MouseEventKind::ScrollUp if m.modifiers.contains(KeyModifiers::CONTROL) => {
            adjust_zoom(app, 1, false);
        }
        MouseEventKind::ScrollDown if m.modifiers.contains(KeyModifiers::CONTROL) => {
            adjust_zoom(app, -1, false);
        }
        MouseEventKind::ScrollUp => {
            app.focus = Focus::Results;
            if app.surface == Surface::Tree && app.folders_area.contains(pos) {
                app.browser_pane = BrowserPane::Folders;
                app.folder_selected = app.folder_selected.saturating_sub(3);
                app.ensure_browser_visible();
                app.preview_folder();
            } else if app.surface == Surface::Tree && app.items_area.contains(pos) {
                app.browser_pane = BrowserPane::Items;
                app.item_selected = app.item_selected.saturating_sub(3);
                app.ensure_browser_visible();
            } else {
                let step = if app.zoom.is_grid() { app.grid_cols } else { 3 };
                app.selected = app.selected.saturating_sub(step);
                app.ensure_visible();
            }
        }
        MouseEventKind::ScrollDown => {
            app.focus = Focus::Results;
            if app.surface == Surface::Tree && app.folders_area.contains(pos) {
                app.browser_pane = BrowserPane::Folders;
                app.folder_selected =
                    (app.folder_selected + 3).min(app.browser_folders.len().saturating_sub(1));
                app.ensure_browser_visible();
                app.preview_folder();
            } else if app.surface == Surface::Tree && app.items_area.contains(pos) {
                app.browser_pane = BrowserPane::Items;
                app.item_selected =
                    (app.item_selected + 3).min(app.browser_items.len().saturating_sub(1));
                app.ensure_browser_visible();
            } else {
                let step = if app.zoom.is_grid() { app.grid_cols } else { 3 };
                app.selected = (app.selected + step).min(app.rows.len().saturating_sub(1));
                app.ensure_visible();
            }
        }
        MouseEventKind::Moved => {
            if app.dragging_bar {
                bar_jump(app, m.row);
                return;
            }
            app.weight_hover = app
                .weight_hits
                .iter()
                .position(|hit| hit.area.contains(pos));
            app.scroll_hover = scrollbar_grab(app.scroll_bar).contains(pos)
                || scrollbar_grab(app.folders_bar).contains(pos)
                || scrollbar_grab(app.items_bar).contains(pos);
            app.preview_hover = app.preview_divider.contains(pos);
            app.browser_hover = if app.surface == Surface::Tree && app.folders_area.contains(pos) {
                Some((
                    BrowserPane::Folders,
                    app.folder_scroll + m.row.saturating_sub(app.folders_area.y) as usize,
                ))
            } else if app.surface == Surface::Tree && app.items_area.contains(pos) {
                Some((
                    BrowserPane::Items,
                    app.item_scroll + m.row.saturating_sub(app.items_area.y) as usize,
                ))
            } else {
                None
            };
            if app.surface != Surface::Tree && app.hits_area.contains(pos) {
                app.hover = result_at(app, pos);
            } else {
                app.hover = None;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if app.dragging_preview => {
            resize_preview(app, m.column);
        }
        MouseEventKind::Drag(MouseButton::Left) if app.dragging_bar => {
            bar_jump(app, m.row);
        }
        MouseEventKind::Up(_) => {
            if app.dragging_preview {
                save_ui_settings(app);
            }
            app.dragging_preview = false;
            app.dragging_bar = false;
        }
        MouseEventKind::Down(MouseButton::Left) if app.preview_divider.contains(pos) => {
            app.dragging_preview = true;
            resize_preview(app, m.column);
        }
        MouseEventKind::Down(MouseButton::Left) if app.weight_mode_area.contains(pos) => {
            app.weight_mode = app.weight_mode.cycle();
            app.weight_hover = None;
        }
        MouseEventKind::Down(MouseButton::Left) if app.weight_area.contains(pos) => {
            if let Some(hit) = app
                .weight_hits
                .iter()
                .find(|hit| hit.area.contains(pos))
                .cloned()
            {
                if hit.path.starts_with('.') {
                    app.query = hit.path;
                    app.surface = Surface::Auto;
                    app.focus = Focus::Search;
                    app.mark_dirty();
                } else if hit.is_dir {
                    app.browse(PathBuf::from(hit.path));
                } else if app.surface == Surface::Tree {
                    if let Some(i) = app
                        .browser_items
                        .iter()
                        .position(|row| row.path.to_string_lossy().as_ref() == hit.path)
                    {
                        app.browser_pane = BrowserPane::Items;
                        app.item_selected = i;
                        app.ensure_browser_visible();
                    }
                } else if let Some(i) = app
                    .rows
                    .iter()
                    .position(|row| row.path.to_string_lossy().as_ref() == hit.path)
                {
                    app.selected = i;
                    app.ensure_visible();
                }
            }
        }
        MouseEventKind::Down(MouseButton::Right)
            if app.surface == Surface::Tree && app.items_area.contains(pos) =>
        {
            let i = app.item_scroll + m.row.saturating_sub(app.items_area.y) as usize;
            if i < app.browser_items.len() {
                app.focus = Focus::Results;
                app.browser_pane = BrowserPane::Items;
                app.item_selected = i;
                app.overlays.push(overlay::Layer::Menu {
                    col: m.column,
                    row: m.row,
                    idx: i,
                    pick: 0,
                });
            }
        }
        MouseEventKind::Down(MouseButton::Right)
            if app.surface != Surface::Tree && app.hits_area.contains(pos) =>
        {
            if let Some(i) = result_at(app, pos) {
                app.focus = Focus::Results;
                app.selected = i;
                app.overlays.push(overlay::Layer::Menu {
                    col: m.column,
                    row: m.row,
                    idx: i,
                    pick: 0,
                });
            }
        }
        MouseEventKind::Down(MouseButton::Left)
            if app.surface == Surface::Tree && scrollbar_grab(app.folders_bar).contains(pos) =>
        {
            app.focus = Focus::Results;
            app.browser_pane = BrowserPane::Folders;
            app.dragging_bar = true;
            bar_jump(app, m.row);
        }
        MouseEventKind::Down(MouseButton::Left)
            if app.surface == Surface::Tree && scrollbar_grab(app.items_bar).contains(pos) =>
        {
            app.focus = Focus::Results;
            app.browser_pane = BrowserPane::Items;
            app.dragging_bar = true;
            bar_jump(app, m.row);
        }
        MouseEventKind::Down(MouseButton::Left)
            if app.surface != Surface::Tree && scrollbar_grab(app.scroll_bar).contains(pos) =>
        {
            app.focus = Focus::Results;
            app.dragging_bar = true;
            bar_jump(app, m.row);
        }
        MouseEventKind::Down(MouseButton::Left) if app.header_area.contains(pos) => {
            if m.row != app.header_area.bottom().saturating_sub(1) {
                return;
            }
            for (x0, x1, hit) in &app.header_hot {
                if m.column >= *x0 && m.column < *x1 {
                    match hit {
                        ChipHit::Brand | ChipHit::Skin => {
                            let i = Theme::ALL
                                .iter()
                                .position(|t| t.id == app.theme.id)
                                .unwrap_or(0);
                            app.overlays.toggle_theme(i, app.theme);
                        }
                        ChipHit::Match => {
                            app.match_mode = app.match_mode.cycle();
                            app.status = format!("match  {}", app.match_mode.as_str());
                            app.mark_dirty();
                        }
                        ChipHit::Sort => {
                            app.sort = next_sort(app.sort);
                            app.mark_dirty();
                        }
                        ChipHit::Surface => {
                            if app.surface == Surface::Tree {
                                app.surface = Surface::Auto;
                            } else {
                                set_grid(app, !app.zoom.is_grid());
                            }
                        }
                        ChipHit::Scope => {
                            app.scope = if app.scope == Scope::Folders {
                                Scope::All
                            } else {
                                Scope::Folders
                            };
                            app.mark_dirty();
                        }
                    }
                    return;
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) if app.footer_area.contains(pos) => {
            app.overlays.toggle_help();
        }
        MouseEventKind::Down(MouseButton::Left) if app.prompt_area.contains(pos) => {
            app.focus = Focus::Search;
        }
        MouseEventKind::Down(MouseButton::Left)
            if app.surface == Surface::Tree && app.folders_area.contains(pos) =>
        {
            let i = app.folder_scroll + m.row.saturating_sub(app.folders_area.y) as usize;
            if i < app.browser_folders.len() {
                let dbl = double_click(app, m.column, m.row);
                app.focus = Focus::Results;
                app.browser_pane = BrowserPane::Folders;
                app.folder_selected = i;
                app.preview_folder();
                if dbl && let Some(path) = app.browser_dir.clone() {
                    app.browse(path);
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left)
            if app.surface == Surface::Tree && app.items_area.contains(pos) =>
        {
            let i = app.item_scroll + m.row.saturating_sub(app.items_area.y) as usize;
            if i < app.browser_items.len() {
                let dbl = double_click(app, m.column, m.row);
                app.focus = Focus::Results;
                app.browser_pane = BrowserPane::Items;
                app.item_selected = i;
                if dbl {
                    app.enter_selected();
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) if app.hits_area.contains(pos) => {
            if let Some(i) = result_at(app, pos) {
                let dbl = double_click(app, m.column, m.row);
                app.focus = Focus::Results;
                app.selected = i;
                if dbl {
                    app.enter_selected();
                }
            }
        }
        _ => {}
    }
}

fn result_at(app: &App, pos: Position) -> Option<usize> {
    surface::result_at(app, pos)
}

fn double_click(app: &mut App, column: u16, row: u16) -> bool {
    let now = Instant::now();
    let double = app.last_click.is_some_and(|(t, x, y)| {
        t.elapsed() < Duration::from_millis(400) && x.abs_diff(column) <= 1 && y.abs_diff(row) <= 1
    });
    app.last_click = Some((now, column, row));
    double
}

fn scrollbar_grab(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_sub(1),
        area.y,
        area.width.saturating_add(1),
        area.height,
    )
}

fn overlay_key(app: &mut App, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
    {
        return true;
    }
    match key.code {
        KeyCode::Esc | KeyCode::F(1)
            if matches!(app.overlays.top(), Some(overlay::Layer::Help)) =>
        {
            app.overlays.pop();
        }
        KeyCode::F(1) => app.overlays.toggle_help(),
        KeyCode::F(8) => {
            if let Some(overlay::Layer::Theme { original, .. }) = app.overlays.pop() {
                app.theme = original;
            }
        }
        KeyCode::Esc => {
            if let Some(overlay::Layer::Theme { original, .. }) = app.overlays.pop() {
                app.theme = original;
            }
        }
        KeyCode::Up => {
            let selected = match app.overlays.top_mut() {
                Some(overlay::Layer::Theme { selected, .. }) => {
                    *selected = selected.saturating_sub(1);
                    Some(*selected)
                }
                Some(overlay::Layer::Menu { pick, .. }) => {
                    *pick = pick.saturating_sub(1);
                    None
                }
                Some(overlay::Layer::Settings { selected }) => {
                    *selected = selected.saturating_sub(1);
                    None
                }
                _ => None,
            };
            if let Some(selected) = selected {
                app.theme = Theme::ALL[selected];
            }
        }
        KeyCode::Down => {
            let selected = match app.overlays.top_mut() {
                Some(overlay::Layer::Theme { selected, .. }) => {
                    *selected = (*selected + 1).min(Theme::ALL.len().saturating_sub(1));
                    Some(*selected)
                }
                Some(overlay::Layer::Menu { pick, .. }) => {
                    *pick = (*pick + 1).min(overlay::MENU_ITEMS.len().saturating_sub(1));
                    None
                }
                Some(overlay::Layer::Settings { selected }) => {
                    *selected = (*selected + 1).min(overlay::SETTINGS_ITEMS - 1);
                    None
                }
                _ => None,
            };
            if let Some(selected) = selected {
                app.theme = Theme::ALL[selected];
            }
        }
        KeyCode::Left | KeyCode::Right => {
            let selected = match app.overlays.top() {
                Some(overlay::Layer::Settings { selected }) => Some(*selected),
                _ => None,
            };
            if let Some(selected) = selected {
                adjust_setting(
                    app,
                    selected,
                    if key.code == KeyCode::Left { -1 } else { 1 },
                    false,
                );
            }
        }
        KeyCode::Enter => match app.overlays.top().cloned() {
            Some(overlay::Layer::Theme { selected, .. }) => {
                if let Some(skin) = Theme::ALL.get(selected).copied() {
                    app.theme = skin;
                    save_theme(app, skin);
                }
                app.overlays.pop();
            }
            Some(overlay::Layer::Menu { idx, pick, .. }) => {
                if app.surface == Surface::Tree {
                    app.item_selected = idx;
                    app.browser_pane = BrowserPane::Items;
                } else {
                    app.selected = idx;
                }
                app.overlays.pop();
                match pick {
                    0 => app.open_selected(),
                    1 => app.preview_selected(),
                    2 => app.copy_selected(),
                    3 => app.reveal_selected(),
                    _ => {}
                }
            }
            Some(overlay::Layer::Settings { selected }) => {
                adjust_setting(app, selected, 1, true);
            }
            _ => {
                app.overlays.pop();
            }
        },
        _ => {}
    }
    false
}

fn save_theme(app: &mut App, skin: Theme) {
    let mut cfg = Config::load();
    cfg.theme = skin.id.into();
    app.status = match cfg.save() {
        Ok(()) => format!("theme  {}", skin.id),
        Err(err) => format!("theme previewed, but config was not saved: {err}"),
    };
}

fn save_ui_settings(app: &mut App) {
    let mut cfg = Config::load();
    cfg.zoom = app.zoom.get();
    cfg.zebra = app.zebra;
    cfg.preview_width = app.preview_width;
    cfg.weight_map = app.show_weight;
    cfg.open = app.open;
    cfg.theme = app.theme.id.into();
    app.status = match cfg.save() {
        Ok(()) => "settings saved".into(),
        Err(err) => format!("settings not saved: {err}"),
    };
}

fn save_catalog_settings(app: &mut App) -> bool {
    let mut cfg = Config::load();
    cfg.show_hidden = app.show_hidden;
    cfg.respect_gitignore = app.respect_gitignore;
    cfg.respect_ignore = app.respect_ignore;
    cfg.exclude_paths.clone_from(&app.excluded_paths);
    match cfg.save() {
        Ok(()) => true,
        Err(error) => {
            app.status = format!("settings not saved: {error}");
            false
        }
    }
}

fn selected_folder(app: &App) -> Option<PathBuf> {
    let row = app.selected_row()?;
    let path = if row.is_dir {
        row.path.clone()
    } else {
        row.path.parent()?.to_path_buf()
    };
    Some(path.canonicalize().unwrap_or(path))
}

fn toggle_selected_folder(app: &mut App) {
    let Some(folder) = selected_folder(app) else {
        app.status = "select a file or folder first".into();
        return;
    };
    let previous = app.excluded_paths.clone();
    if let Some(index) = app.excluded_paths.iter().position(|path| path == &folder) {
        app.excluded_paths.remove(index);
    } else {
        app.excluded_paths.push(folder);
    }
    if save_catalog_settings(app) {
        app.request_catalog_refresh();
    } else {
        app.excluded_paths = previous;
    }
}

fn set_grid(app: &mut App, grid: bool) {
    if app.surface == Surface::Auto && app.zoom.is_grid() == grid {
        return;
    }
    app.surface = Surface::Auto;
    app.zoom = Zoom::new(if grid { 70 } else { 12 });
    app.invalidate_thumbnail_jobs();
    save_ui_settings(app);
}

fn adjust_zoom(app: &mut App, direction: i8, stay_grid: bool) {
    let next = app.zoom.bump(direction);
    let next = if stay_grid && !next.is_grid() {
        Zoom::new(Zoom::GRID_FROM)
    } else {
        next
    };
    if app.surface == Surface::Auto && app.zoom == next {
        return;
    }
    app.surface = Surface::Auto;
    app.zoom = next;
    app.invalidate_thumbnail_jobs();
    save_ui_settings(app);
}

fn open_theme_picker(app: &mut App) {
    let selected = Theme::ALL
        .iter()
        .position(|theme| theme.id == app.theme.id)
        .unwrap_or(0);
    app.overlays.toggle_theme(selected, app.theme);
}

fn adjust_setting(app: &mut App, index: usize, direction: i8, activate: bool) {
    match index {
        0 if activate => open_theme_picker(app),
        0 => {
            let current = Theme::ALL
                .iter()
                .position(|theme| theme.id == app.theme.id)
                .unwrap_or(0) as isize;
            let next = (current + isize::from(direction)).rem_euclid(Theme::ALL.len() as isize);
            app.theme = Theme::ALL[next as usize];
            save_theme(app, app.theme);
        }
        1 if activate => set_grid(app, !app.zoom.is_grid()),
        1 => adjust_zoom(app, direction, app.zoom.is_grid()),
        2 => {
            let delta = if direction < 0 { -5 } else { 5 };
            app.preview_width = (i16::from(app.preview_width) + delta).clamp(20, 70) as u8;
            save_ui_settings(app);
        }
        3 => {
            app.zebra = !app.zebra;
            save_ui_settings(app);
        }
        4 => {
            let states = [
                (false, WeightMode::Size),
                (true, WeightMode::Size),
                (true, WeightMode::Format),
            ];
            let current = states
                .iter()
                .position(|state| *state == (app.show_weight, app.weight_mode))
                .unwrap_or(0) as isize;
            let delta = if direction < 0 { -1 } else { 1 };
            let next = (current + delta).rem_euclid(states.len() as isize) as usize;
            (app.show_weight, app.weight_mode) = states[next];
            save_ui_settings(app);
        }
        5 => {
            app.open = if direction < 0 {
                match app.open {
                    OpenMode::Auto => OpenMode::Editor,
                    OpenMode::Xdg => OpenMode::Auto,
                    OpenMode::Editor => OpenMode::Xdg,
                }
            } else {
                app.open.cycle()
            };
            save_ui_settings(app);
        }
        6 => {
            app.show_hidden = !app.show_hidden;
            if save_catalog_settings(app) {
                if app.surface == Surface::Tree {
                    app.reload_browser();
                }
                app.mark_dirty();
            } else {
                app.show_hidden = !app.show_hidden;
            }
        }
        7 => {
            app.respect_gitignore = !app.respect_gitignore;
            if save_catalog_settings(app) {
                app.query_session = query::Session::new(
                    app.catalog.clone(),
                    app.events.clone(),
                    app.respect_gitignore,
                    app.respect_ignore,
                );
                app.mark_dirty();
            } else {
                app.respect_gitignore = !app.respect_gitignore;
            }
        }
        8 => {
            app.respect_ignore = !app.respect_ignore;
            if save_catalog_settings(app) {
                app.query_session = query::Session::new(
                    app.catalog.clone(),
                    app.events.clone(),
                    app.respect_gitignore,
                    app.respect_ignore,
                );
                app.mark_dirty();
            } else {
                app.respect_ignore = !app.respect_ignore;
            }
        }
        9 if activate => toggle_selected_folder(app),
        _ => {}
    }
}

fn resize_preview(app: &mut App, column: u16) {
    let width = app.content_area.width.max(1);
    let preview = app.content_area.right().saturating_sub(column).min(width);
    app.preview_width = ((u32::from(preview) * 100 / u32::from(width)) as u8).clamp(20, 70);
}

fn bar_jump(app: &mut App, row: u16) {
    surface::bar_jump(app, row);
}

fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if !app.overlays.is_empty() {
        return overlay_key(app, key);
    }
    if app.surface == Surface::Tree {
        match key.code {
            KeyCode::Char(' ') if app.focus == Focus::Results => {
                app.preview_selected();
                return false;
            }
            KeyCode::Enter => {
                app.enter_selected();
                return false;
            }
            KeyCode::Tab => {
                app.focus = match app.focus {
                    Focus::Search => Focus::Results,
                    Focus::Results => Focus::Search,
                };
                return false;
            }
            KeyCode::Left | KeyCode::Right => {
                app.focus = Focus::Results;
                app.browser_pane = match app.browser_pane {
                    BrowserPane::Folders => BrowserPane::Items,
                    BrowserPane::Items => BrowserPane::Folders,
                };
                app.ensure_browser_visible();
                return false;
            }
            KeyCode::Down | KeyCode::Char('n')
                if key.code == KeyCode::Down || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                app.focus = Focus::Results;
                match app.browser_pane {
                    BrowserPane::Folders => {
                        app.folder_selected = (app.folder_selected + 1)
                            .min(app.browser_folders.len().saturating_sub(1));
                        app.preview_folder();
                    }
                    BrowserPane::Items => {
                        app.item_selected =
                            (app.item_selected + 1).min(app.browser_items.len().saturating_sub(1));
                    }
                }
                app.ensure_browser_visible();
                return false;
            }
            KeyCode::Up | KeyCode::Char('p')
                if key.code == KeyCode::Up || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                app.focus = Focus::Results;
                match app.browser_pane {
                    BrowserPane::Folders => {
                        app.folder_selected = app.folder_selected.saturating_sub(1);
                        app.preview_folder();
                    }
                    BrowserPane::Items => {
                        app.item_selected = app.item_selected.saturating_sub(1);
                    }
                }
                app.ensure_browser_visible();
                return false;
            }
            KeyCode::PageDown | KeyCode::PageUp => {
                app.focus = Focus::Results;
                let down = key.code == KeyCode::PageDown;
                let step = match app.browser_pane {
                    BrowserPane::Folders => app.folders_area.height as usize,
                    BrowserPane::Items => app.items_area.height as usize,
                }
                .max(1);
                match app.browser_pane {
                    BrowserPane::Folders => {
                        app.folder_selected = if down {
                            (app.folder_selected + step)
                                .min(app.browser_folders.len().saturating_sub(1))
                        } else {
                            app.folder_selected.saturating_sub(step)
                        };
                        app.preview_folder();
                    }
                    BrowserPane::Items => {
                        app.item_selected = if down {
                            (app.item_selected + step)
                                .min(app.browser_items.len().saturating_sub(1))
                        } else {
                            app.item_selected.saturating_sub(step)
                        };
                    }
                }
                app.ensure_browser_visible();
                return false;
            }
            KeyCode::Backspace if app.query.is_empty() => {
                if let Some(parent) = app
                    .browser_root
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                {
                    app.browse(parent);
                }
                return false;
            }
            KeyCode::Backspace => {
                app.surface = Surface::Auto;
                app.focus = Focus::Search;
                app.query.pop();
                app.mark_dirty();
                return false;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.surface = Surface::Auto;
                app.focus = Focus::Search;
                app.query.push(c);
                app.mark_dirty();
                return false;
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Char('c' | 'q') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Esc => return true,
        KeyCode::F(1) => app.overlays.toggle_help(),
        KeyCode::Enter => app.enter_selected(),
        KeyCode::Char(' ') if app.focus == Focus::Results => app.preview_selected(),
        KeyCode::F(3) => app.preview_selected(),
        KeyCode::F(4) => {
            if app.surface == Surface::Tree {
                app.surface = Surface::Auto;
            } else {
                app.browse_from_selection();
            }
        }
        KeyCode::F(6) => adjust_setting(app, 4, 1, false),
        KeyCode::F(8) => app.overlays.toggle_settings(),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cycle_open();
        }
        KeyCode::Char('=') | KeyCode::Char('+')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            adjust_zoom(app, 1, false);
        }
        KeyCode::Char('-') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            adjust_zoom(app, -1, false);
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reveal_selected();
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.copy_selected();
        }
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
            app.focus = Focus::Search;
            app.query.pop();
            app.mark_dirty();
        }
        KeyCode::Down | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.focus = Focus::Results;
            if !app.rows.is_empty() {
                let step = if app.zoom.is_grid() { app.grid_cols } else { 1 };
                app.selected = (app.selected + step).min(app.rows.len() - 1);
            }
            app.ensure_visible();
        }
        KeyCode::Down => {
            app.focus = Focus::Results;
            if !app.rows.is_empty() {
                let step = if app.zoom.is_grid() { app.grid_cols } else { 1 };
                app.selected = (app.selected + step).min(app.rows.len() - 1);
            }
            app.ensure_visible();
        }
        KeyCode::Up | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.focus = Focus::Results;
            let step = if app.zoom.is_grid() { app.grid_cols } else { 1 };
            app.selected = app.selected.saturating_sub(step);
            app.ensure_visible();
        }
        KeyCode::Up => {
            app.focus = Focus::Results;
            let step = if app.zoom.is_grid() { app.grid_cols } else { 1 };
            app.selected = app.selected.saturating_sub(step);
            app.ensure_visible();
        }
        KeyCode::PageDown => {
            app.focus = Focus::Results;
            app.selected = (app.selected + app.view_h.max(1)).min(app.rows.len().saturating_sub(1));
            app.ensure_visible();
        }
        KeyCode::PageUp => {
            app.focus = Focus::Results;
            app.selected = app.selected.saturating_sub(app.view_h.max(1));
            app.ensure_visible();
        }
        KeyCode::Left if app.focus == Focus::Results && app.zoom.is_grid() => {
            app.selected = app.selected.saturating_sub(1);
            app.ensure_visible();
        }
        KeyCode::Right if app.focus == Focus::Results && app.zoom.is_grid() => {
            app.selected = (app.selected + 1).min(app.rows.len().saturating_sub(1));
            app.ensure_visible();
        }
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Search => Focus::Results,
                Focus::Results => Focus::Search,
            };
        }
        KeyCode::Char('+') if app.focus == Focus::Results => adjust_zoom(app, 1, true),
        KeyCode::Char('-') if app.focus == Focus::Results => adjust_zoom(app, -1, true),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.focus = Focus::Search;
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

fn spawn_detached(command: &mut Command) -> std::io::Result<std::process::Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn launch_editor(program: &str, args: &[String], path: &Path) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args).arg(path);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(error).with_context(|| format!("open {} with {program}", path.display()))
    }
    #[cfg(not(unix))]
    {
        spawn_detached(&mut command)
            .with_context(|| format!("open {} with {program}", path.display()))?;
        Ok(())
    }
}

fn desktop_open(path: &Path) -> (&'static str, std::io::Result<std::process::Child>) {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", "start", ""]).arg(path);
        return ("open", spawn_detached(&mut command));
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(path);
        return ("open", spawn_detached(&mut command));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        ("xdg-open", spawn_detached(&mut command))
    }
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
    let th = app.theme;
    frame.render_widget(Block::default().style(Style::default().bg(th.bg)), area);
    draw_header(frame, app.header_area, app);
    match app.surface {
        Surface::Tree => draw_tree(frame, app.content_area, app),
        Surface::Auto if app.preview_area.width > 0 => {
            if app.zoom.is_grid() {
                draw_grid(frame, app.hits_area, app);
            } else {
                draw_list(frame, app.hits_area, app);
            }
            draw_side_preview(frame, app.preview_area, app);
        }
        Surface::Auto if app.zoom.is_grid() => draw_grid(frame, app.hits_area, app),
        Surface::Auto => draw_list(frame, app.hits_area, app),
    }
    if app.show_weight {
        draw_weight(frame, app.weight_panel_area, app);
    }
    draw_prompt(frame, app.prompt_area, app);
    draw_footer(frame, app.footer_area, app);

    let view_setting = if app.zoom.is_grid() {
        format!("grid · {}", app.zoom.get())
    } else {
        format!("list · {}", app.zoom.get())
    };
    let preview_setting = format!("{}%", app.preview_width);
    let selected_folder_state = selected_folder(app).map_or("none", |folder| {
        if app.excluded_paths.iter().any(|path| path == &folder) {
            "excluded"
        } else {
            "indexed"
        }
    });
    let settings = [
        ("Theme", app.theme.id),
        ("View", view_setting.as_str()),
        ("Preview width", preview_setting.as_str()),
        ("Row contrast", if app.zebra { "zebra" } else { "plain" }),
        ("Weight map", weight_setting_label(app)),
        ("Open files", app.open.as_str()),
        ("Show hidden", if app.show_hidden { "on" } else { "off" }),
        (
            "Git ignore rules",
            if app.respect_gitignore { "on" } else { "off" },
        ),
        (
            ".ignore rules",
            if app.respect_ignore { "on" } else { "off" },
        ),
        ("Selected folder", selected_folder_state),
    ];
    overlay::draw(frame, &app.overlays, th, area, &settings);
}

fn weight_setting_label(app: &App) -> &'static str {
    match (app.show_weight, app.weight_mode) {
        (false, _) => "off",
        (true, WeightMode::Size) => "size",
        (true, WeightMode::Format) => "file types",
    }
}

fn draw_side_preview(frame: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let Some(row) = app.rows.get(app.selected) else {
        frame.render_widget(Block::default().style(Style::new().bg(th.surface)), area);
        return;
    };
    let kind = if row.is_dir {
        format!("folder · {} items", app.previews.side.body.len())
    } else {
        human_size(row.size)
    };
    let header = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} ", icon_for(&row.path, row.is_dir)),
                Style::new()
                    .fg(th.bg)
                    .bg(th.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                row.name.clone(),
                Style::new().fg(th.text).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(format!("    {kind}"), Style::new().fg(th.dim))),
        Line::from(Span::styled(
            format!("    {}", row.path.display()),
            Style::new().fg(th.dim),
        )),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::new().fg(if app.preview_hover || app.dragging_preview {
                th.accent
            } else {
                th.border
            }),
        )
        .style(Style::new().bg(th.surface))
        .title(Span::styled(
            " PREVIEW ",
            Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let panes = Layout::vertical([Constraint::Length(4), Constraint::Min(1)]).split(inner);
    frame.render_widget(
        Paragraph::new(header)
            .wrap(Wrap { trim: false })
            .style(Style::new().bg(th.surface)),
        panes[0],
    );
    if app.previews.side.kind == preview::Kind::Image && app.previews.side.body.is_empty() {
        frame.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            panes[1],
            &mut app.previews.image,
        );
    } else {
        let text_area = Rect::new(
            panes[1].x,
            panes[1].y,
            panes[1].width.saturating_sub(1),
            panes[1].height,
        );
        frame.render_widget(
            side_preview_paragraph(app).scroll((app.preview_scroll, 0)),
            text_area,
        );
        if app.preview_max_scroll > 0 {
            draw_scroll(
                frame,
                Rect::new(
                    panes[1].right().saturating_sub(1),
                    panes[1].y,
                    1,
                    panes[1].height,
                ),
                app.preview_scroll as usize,
                app.preview_max_scroll as usize + panes[1].height as usize,
                panes[1].height as usize,
                th,
                false,
            );
        }
    }
}

fn side_preview_paragraph(app: &App) -> Paragraph<'static> {
    Paragraph::new(side_preview_lines(app))
        .wrap(Wrap { trim: false })
        .style(Style::new().bg(app.theme.surface))
}

fn side_preview_lines(app: &App) -> Vec<Line<'static>> {
    let th = app.theme;
    if app.previews.side.kind == preview::Kind::Markdown {
        markdown_preview(&app.previews.side.body, th)
    } else {
        app.previews
            .side
            .body
            .iter()
            .map(|line| Line::from(Span::styled(format!("  {line}"), Style::new().fg(th.text))))
            .collect()
    }
}

fn markdown_preview(body: &[String], th: Theme) -> Vec<Line<'static>> {
    let mut code = false;
    let mut lines = Vec::with_capacity(body.len());
    for raw in body {
        let trimmed = raw.trim_start();
        if let Some(language) = trimmed.strip_prefix("```") {
            code = !code;
            if !language.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {language}"),
                    Style::new().fg(th.dim).bg(th.surface),
                )));
            }
        } else if code {
            lines.push(Line::from(Span::styled(
                format!("  {raw}"),
                Style::new().fg(th.sky).bg(th.bg),
            )));
        } else if let Some(text) = markdown_heading(trimmed) {
            lines.push(Line::from(Span::styled(
                format!("  {text}"),
                Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
            )));
        } else if let Some(text) = trimmed.strip_prefix("> ") {
            lines.push(Line::from(Span::styled(
                format!("  │ {text}"),
                Style::new().fg(th.purple),
            )));
        } else if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let (mark, text) = if let Some(text) = text.strip_prefix("[x] ") {
                ("✓", text)
            } else if let Some(text) = text.strip_prefix("[ ] ") {
                ("○", text)
            } else {
                ("•", text)
            };
            let mut spans = vec![Span::styled(
                format!("  {mark} "),
                Style::new().fg(th.accent),
            )];
            spans.extend(markdown_inline(text, th));
            lines.push(Line::from(spans));
        } else if matches!(trimmed, "---" | "***" | "___") {
            lines.push(Line::from(Span::styled(
                "  ─────────────────────────",
                Style::new().fg(th.border),
            )));
        } else {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(markdown_inline(raw, th));
            lines.push(Line::from(spans));
        }
    }
    lines
}

fn markdown_heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line.get(hashes..)?.strip_prefix(' '))
        .flatten()
}

fn markdown_inline(text: &str, th: Theme) -> Vec<Span<'static>> {
    text.split("**")
        .enumerate()
        .flat_map(|(bold, part)| {
            part.split('`').enumerate().map(move |(code, text)| {
                Span::styled(
                    text.to_owned(),
                    Style::new()
                        .fg(if code % 2 == 1 { th.sky } else { th.text })
                        .bg(if code % 2 == 1 { th.bg } else { th.surface })
                        .add_modifier(if bold % 2 == 1 {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                )
            })
        })
        .collect()
}

fn surface_label(app: &App) -> String {
    if app.surface == Surface::Tree {
        "browse".into()
    } else if app.zoom.is_grid() {
        format!("grid {}", app.zoom.get())
    } else {
        format!("list {}", app.zoom.get())
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let mut chips = vec![
        Chip::new("QFIND", th.bg, th.accent),
        Chip::new(
            format!("{} files", compact(app.catalog.file_count())),
            th.text,
            th.surface,
        ),
    ];
    let mut hits: Vec<Option<ChipHit>> = vec![Some(ChipHit::Brand), None];
    if area.width >= 48 {
        chips.push(Chip::new(app.match_mode.as_str(), th.accent, th.select_bg));
        hits.push(Some(ChipHit::Match));
    }
    if area.width >= 60 {
        chips.push(Chip::new(sort_label(app.sort), th.text, th.surface));
        hits.push(Some(ChipHit::Sort));
    }
    if area.width >= 74 {
        chips.push(Chip::new(surface_label(app), th.bg, th.purple));
        hits.push(Some(ChipHit::Surface));
    }
    if area.width >= 90 {
        let scope = if app.scope == Scope::Folders {
            "folders"
        } else {
            "all"
        };
        chips.push(Chip::new(scope, th.accent, th.surface));
        hits.push(Some(ChipHit::Scope));
    }
    if area.width >= 104 && app.class != FileClass::All {
        chips.push(Chip::new(class_label(app.class), th.text, th.surface));
        hits.push(None);
    }
    if area.width >= 118 {
        chips.push(Chip::new(th.id, th.dim, th.surface));
        hits.push(Some(ChipHit::Skin));
    }
    let chips = fit_chips(chips, area.width);
    app.header_hot.clear();
    let mut x = area.x;
    for (i, chip) in chips.iter().enumerate() {
        let w = (chip.text.chars().count() as u16).saturating_add(3);
        if let Some(Some(hit)) = hits.get(i) {
            app.header_hot.push((x, x.saturating_add(w), *hit));
        }
        x = x.saturating_add(w);
    }
    let rail = Line::from(
        (0..area.width)
            .map(|x| {
                let t = f32::from(x) / f32::from(area.width.max(1));
                Span::styled("━", Style::new().fg(th.glow(t)).bg(th.bg))
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(Paragraph::new(rail).style(Style::new().bg(th.bg)), area);
    if area.height > 1 {
        frame.render_widget(
            Paragraph::new(toolbar(&chips, area.width, th.bg)),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
}

fn draw_prompt(frame: &mut Frame, area: Rect, app: &App) {
    let th = app.theme;
    let active = app.focus == Focus::Search;
    let bg = if active { th.surface } else { th.bg };
    let prompt = format!(" {}  ", icon_prompt());
    let query = if app.query.is_empty() {
        Span::styled(
            "Search files, folders, or extensions",
            Style::new().fg(th.dim).bg(bg),
        )
    } else {
        Span::styled(app.query.clone(), Style::new().fg(th.text).bg(bg))
    };
    let cursor = if app.query.is_empty() { "" } else { "▏" };
    let line = Line::from(vec![
        Span::styled(
            prompt,
            Style::new()
                .fg(th.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        query,
        Span::styled(
            cursor,
            Style::new()
                .fg(th.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::new().bg(bg)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::new().bg(bg)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(if active { th.accent } else { th.border }))
                .style(Style::new().bg(bg))
                .title(Span::styled(
                    " SEARCH ",
                    Style::new()
                        .fg(if active { th.accent } else { th.dim })
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let list = area;
    let height = app.view_h;
    let start = app.scroll.min(app.rows.len());
    let end = (start + height).min(app.rows.len());
    frame.render_widget(Block::default().style(Style::new().bg(th.bg)), area);
    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in app.rows.get(start..end).into_iter().flatten().enumerate() {
        let idx = start + i;
        let selected = idx == app.selected;
        let hovered = app.hover == Some(idx) && !selected;
        let zebra = app.zebra && idx % 2 == 1;
        lines.push(row_line(
            row,
            selected,
            hovered,
            zebra,
            app.focus == Focus::Results,
            list.width,
            th,
        ));
    }
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(th.bg)), list);
    draw_scroll(
        frame,
        app.scroll_bar,
        app.scroll,
        app.rows.len(),
        height,
        th,
        app.scroll_hover || app.dragging_bar,
    );
}

fn draw_grid(frame: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    frame.render_widget(Block::default().style(Style::new().bg(th.bg)), area);
    let cell_w = app.grid_cell_w;
    let cell_h = app.grid_cell_h;
    let cols = app.grid_cols;
    let start = app.list_start;
    for i in start..(start + app.view_h).min(app.rows.len()) {
        let (name, path, is_dir, bytes) = {
            let row = &app.rows[i];
            (row.name.clone(), row.path.clone(), row.is_dir, row.size)
        };
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
        let icon = icon_for(&path, is_dir);
        let row_bg = if app.zebra && r % 2 == 1 {
            th.zebra
        } else {
            th.bg
        };
        let fg = if is_dir { th.accent } else { th.text };
        let (fg, bg) = if selected && app.focus == Focus::Results {
            (th.accent, th.select_bg)
        } else if selected {
            (th.text, th.surface)
        } else if app.hover == Some(i) {
            (fg, th.surface)
        } else {
            (fg, row_bg)
        };
        let tile = Rect::new(x, y, w.saturating_sub(1).max(1), h);
        let block = Block::default().style(Style::new().bg(bg));
        let block = if selected {
            block.borders(Borders::ALL).border_style(Style::new().fg(
                if app.focus == Focus::Results {
                    th.accent
                } else {
                    th.border
                },
            ))
        } else {
            block
        };
        frame.render_widget(block, tile);
        if tile.width <= 2 || tile.height <= 2 {
            continue;
        }
        let inner = Rect::new(tile.x + 1, tile.y + 1, tile.width - 2, tile.height - 2);
        let visual = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );
        if !is_dir && visual.width > 0 && visual.height > 0 {
            let target = visual.as_size();
            if let Some(thumbnail) = app.previews.thumbnail(&path, target) {
                match thumbnail {
                    preview::Tile::Image(protocol) => {
                        let size = protocol.size();
                        let image_area = Rect::new(
                            visual.x + visual.width.saturating_sub(size.width) / 2,
                            visual.y + visual.height.saturating_sub(size.height) / 2,
                            size.width.min(visual.width),
                            size.height.min(visual.height),
                        );
                        frame.render_widget(Image::new(protocol), image_area);
                    }
                    preview::Tile::Text(lines) => {
                        let snippet = lines
                            .iter()
                            .take(visual.height as usize)
                            .map(|line| truncate(line, visual.width as usize))
                            .collect::<Vec<_>>()
                            .join("\n");
                        frame.render_widget(
                            Paragraph::new(snippet)
                                .wrap(Wrap { trim: true })
                                .style(Style::new().fg(th.dim).bg(bg)),
                            visual,
                        );
                    }
                    preview::Tile::Icon => draw_grid_placeholder(frame, visual, icon, fg, bg),
                }
            } else {
                draw_grid_placeholder(frame, visual, "·", th.dim, bg);
            }
        } else {
            draw_grid_placeholder(frame, visual, icon, fg, bg);
        }
        let size = if is_dir {
            "folder".to_owned()
        } else {
            human_size(bytes)
        };
        let name_width = inner
            .width
            .saturating_sub(size.chars().count() as u16)
            .saturating_sub(2) as usize;
        let label = format!("{}  {size}", truncate(&name, name_width));
        frame.render_widget(
            Paragraph::new(label).style(Style::new().fg(fg).bg(bg).add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            })),
            Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
        );
    }
}

fn draw_grid_placeholder(
    frame: &mut Frame,
    area: Rect,
    icon: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(icon)
            .alignment(Alignment::Center)
            .style(Style::new().fg(fg).bg(bg)),
        Rect::new(area.x, area.y + area.height / 2, area.width, 1),
    );
}

fn draw_tree(frame: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    frame.render_widget(Block::default().style(Style::new().bg(th.bg)), area);
    let root = app
        .browser_root
        .as_deref()
        .unwrap_or_else(|| Path::new("/"));
    let folder_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(
            if app.focus == Focus::Results && app.browser_pane == BrowserPane::Folders {
                th.accent
            } else {
                th.border
            },
        ))
        .title(Line::from(vec![
            Span::styled(
                " FOLDERS ",
                Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", root.display()),
                Style::new().fg(th.dim).bg(th.bg),
            ),
        ]));
    frame.render_widget(folder_block.style(Style::new().bg(th.bg)), app.folder_pane);

    let item_title = app
        .browser_dir
        .as_deref()
        .unwrap_or(root)
        .display()
        .to_string();
    let item_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(
            if app.focus == Focus::Results && app.browser_pane == BrowserPane::Items {
                th.accent
            } else {
                th.border
            },
        ))
        .title(Line::from(vec![
            Span::styled(
                " ITEMS ",
                Style::new().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {item_title} "), Style::new().fg(th.dim).bg(th.bg)),
        ]));
    frame.render_widget(item_block.style(Style::new().bg(th.bg)), app.item_pane);
    draw_browser_rows(
        frame,
        app.folders_area,
        &app.browser_folders,
        (
            app.folder_selected,
            app.folder_scroll,
            app.browser_hover
                .and_then(|(pane, i)| (pane == BrowserPane::Folders).then_some(i)),
        ),
        false,
        app.focus == Focus::Results && app.browser_pane == BrowserPane::Folders,
        th,
    );
    draw_browser_rows(
        frame,
        app.items_area,
        &app.browser_items,
        (
            app.item_selected,
            app.item_scroll,
            app.browser_hover
                .and_then(|(pane, i)| (pane == BrowserPane::Items).then_some(i)),
        ),
        true,
        app.focus == Focus::Results && app.browser_pane == BrowserPane::Items,
        th,
    );
    draw_scroll(
        frame,
        app.folders_bar,
        app.folder_scroll,
        app.browser_folders.len(),
        app.folders_area.height as usize,
        th,
        app.browser_pane == BrowserPane::Folders && (app.scroll_hover || app.dragging_bar),
    );
    draw_scroll(
        frame,
        app.items_bar,
        app.item_scroll,
        app.browser_items.len(),
        app.items_area.height as usize,
        th,
        app.browser_pane == BrowserPane::Items && (app.scroll_hover || app.dragging_bar),
    );
}

fn draw_browser_rows(
    frame: &mut Frame,
    area: Rect,
    rows: &[Row],
    selection: (usize, usize, Option<usize>),
    sizes: bool,
    focused: bool,
    th: Theme,
) {
    let (selected, scroll, hovered) = selection;
    let end = (scroll + area.height as usize).min(rows.len());
    let lines = rows
        .get(scroll..end)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(offset, row)| {
            let active = scroll + offset == selected;
            let bg = if active && focused {
                th.select_bg
            } else if active || hovered == Some(scroll + offset) {
                th.surface
            } else {
                th.bg
            };
            let icon = icon_for(&row.path, row.is_dir);
            let marker = if active && focused { "▌" } else { " " };
            let mut label = format!("{marker} {icon} {}", row.name);
            if sizes && !row.is_dir && area.width >= 24 {
                let size = human_size(row.size);
                let pad = area
                    .width
                    .saturating_sub(label.chars().count() as u16)
                    .saturating_sub(size.len() as u16)
                    .max(1);
                label.push_str(&" ".repeat(pad as usize));
                label.push_str(&size);
            }
            Line::from(Span::styled(
                label,
                Style::new()
                    .fg(if row.is_dir { th.accent } else { th.text })
                    .bg(bg)
                    .add_modifier(if active {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(th.bg)), area);
}

fn draw_weight(frame: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let rows = if app.surface == Surface::Tree {
        &app.browser_items
    } else {
        &app.rows
    };
    let weighted = match app.weight_mode {
        WeightMode::Size if app.surface == Surface::Tree => rows
            .iter()
            .map(|row| Weighted {
                name: row.name.clone(),
                path: row.path.to_string_lossy().into_owned(),
                weight: row.size.max(1),
                id: None,
            })
            .collect(),
        WeightMode::Size => folder_weights(
            &rows
                .iter()
                .map(|row| HitRef {
                    id: None,
                    path: row.path.to_string_lossy().into_owned(),
                    is_dir: row.is_dir,
                    weight: row.size.max(1),
                })
                .collect::<Vec<_>>(),
        ),
        WeightMode::Format => format_weights(rows),
    };
    let inner = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .inner(area);
    let mode = app.weight_mode.label().to_uppercase();
    let title_width = " WEIGHT MAP  ".chars().count() + mode.chars().count() + 2;
    app.weight_mode_area = Rect::new(area.x.saturating_add(1), area.y, title_width as u16, 1);
    app.weight_area = inner;
    app.weight_hits.clear();
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(th.border))
            .style(Style::new().bg(th.bg))
            .title(Line::from(vec![
                Span::styled(
                    " WEIGHT MAP ",
                    Style::new().fg(th.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {mode} "),
                    Style::new()
                        .fg(th.bg)
                        .bg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
            .title_bottom(
                Line::from(Span::styled(" click to focus ", Style::new().fg(th.dim)))
                    .right_aligned(),
            ),
        area,
    );
    if weighted.is_empty() {
        frame.render_widget(
            Paragraph::new(" no files in this slice").style(Style::new().fg(th.dim).bg(th.bg)),
            inner,
        );
        return;
    }
    let tiles = squarify(weighted, f64::from(inner.width), f64::from(inner.height));
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
        let tile_area = Rect::new(
            x,
            y,
            w.saturating_sub(u16::from(w > 3)),
            h.saturating_sub(u16::from(h > 2)),
        );
        if tile_area.width == 0 || tile_area.height == 0 {
            continue;
        }
        let hovered = app.weight_hover == Some(i);
        let bg = if hovered { th.accent } else { th.map_tile(i) };
        let is_dir = app.weight_mode == WeightMode::Size
            && (app.surface != Surface::Tree
                || rows
                    .iter()
                    .any(|row| row.is_dir && row.path.to_string_lossy().as_ref() == t.path));
        app.weight_hits.push(WeightHit {
            area: tile_area,
            path: t.path.clone(),
            is_dir,
        });
        let value = match app.weight_mode {
            WeightMode::Size => human_size(t.weight),
            WeightMode::Format => format!("{} files", t.weight),
        };
        let label = if w >= 10 && h >= 2 {
            format!(
                " {}\n {}",
                truncate(&t.name, w.saturating_sub(2) as usize),
                value
            )
        } else {
            format!(" {}", truncate(&t.name, w.saturating_sub(1) as usize))
        };
        frame.render_widget(
            Paragraph::new(label).style(
                Style::new()
                    .bg(bg)
                    .fg(if hovered { th.bg } else { th.text })
                    .add_modifier(Modifier::BOLD),
            ),
            tile_area,
        );
    }
}

fn format_weights(rows: &[Row]) -> Vec<Weighted> {
    let mut groups: BTreeMap<String, u64> = BTreeMap::new();
    for row in rows.iter().filter(|row| !row.is_dir) {
        let ext = row
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|ext| !ext.is_empty())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_else(|| "no extension".into());
        *groups.entry(ext).or_default() += 1;
    }
    let mut weighted: Vec<_> = groups
        .into_iter()
        .map(|(name, weight)| Weighted {
            path: if name == "no extension" {
                String::new()
            } else {
                format!(".{name}")
            },
            name,
            weight,
            id: None,
        })
        .collect();
    weighted.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.name.cmp(&b.name)));
    weighted
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn truncate(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        text.to_string()
    } else if width <= 1 {
        "…".chars().take(width).collect()
    } else {
        format!("{}…", text.chars().take(width - 1).collect::<String>())
    }
}

fn draw_scroll(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    total: usize,
    view: usize,
    th: Theme,
    active: bool,
) {
    if area.width == 0 || area.height == 0 || total <= view {
        return;
    }
    let track = area.height as usize;
    let thumb = ((view * track) / total).max(1);
    let max_off = total.saturating_sub(view);
    let y0 = (scroll * track.saturating_sub(thumb))
        .checked_div(max_off)
        .unwrap_or(0);
    for y in 0..area.height {
        let on = (y as usize) >= y0 && (y as usize) < y0 + thumb;
        frame.render_widget(
            Paragraph::new(" ").style(Style::new().bg(if on {
                if active { th.match_fg } else { th.accent }
            } else {
                th.surface
            })),
            Rect::new(area.x, area.y + y, 1, 1),
        );
    }
}

fn row_line(
    row: &Row,
    selected: bool,
    hovered: bool,
    zebra: bool,
    focused: bool,
    width: u16,
    th: Theme,
) -> Line<'static> {
    let bg = if selected && focused {
        th.select_bg
    } else if selected || hovered {
        th.surface
    } else if zebra {
        th.zebra
    } else {
        th.bg
    };
    let bar = Span::styled(
        if selected && focused {
            "▌ "
        } else if selected || hovered {
            "▎ "
        } else {
            "  "
        },
        Style::new()
            .fg(if selected || hovered { th.accent } else { bg })
            .bg(bg),
    );
    let glyph = icon_for(&row.path, row.is_dir);
    let icon = Span::styled(
        format!("{glyph} "),
        Style::new()
            .fg(if row.is_dir { th.accent } else { th.dim })
            .bg(bg),
    );
    let mut spans = vec![bar, icon];
    spans.extend(highlight_chars(&row.name, &row.indices, bg, th));
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let size = if row.is_dir {
        "folder".to_owned()
    } else {
        human_size(row.size)
    };
    let path = row.path.display().to_string();
    let remain = (width as usize).saturating_sub(used + size.chars().count() + 3);
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
    spans.push(Span::styled(shown, Style::new().fg(th.dim).bg(bg)));
    let pad = (width as usize).saturating_sub(used + shown_cols + size.chars().count() + 1);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Style::new().bg(bg)));
    }
    spans.push(Span::styled(
        format!(" {size}"),
        Style::new()
            .fg(if selected { th.accent } else { th.dim })
            .bg(bg),
    ));
    Line::from(spans)
}

fn highlight_chars(
    name: &str,
    indices: &[u32],
    bg: ratatui::style::Color,
    th: Theme,
) -> Vec<Span<'static>> {
    let set: HashSet<u32> = indices.iter().copied().collect();
    name.chars()
        .enumerate()
        .map(|(i, ch)| {
            let matched = set.contains(&(i as u32));
            let style = if matched {
                Style::new()
                    .bg(bg)
                    .fg(th.match_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().bg(bg).fg(th.text)
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let th = app.theme;
    let status = format!(" {} ", app.status);
    let sw = (status.chars().count() as u16)
        .min(area.width.saturating_sub(8))
        .max(1);
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(8), Constraint::Length(sw)])
        .split(area);
    let mut chips = vec![
        Chip::new(
            match app.focus {
                Focus::Search => "Search focus",
                Focus::Results => "Results focus",
            },
            th.bg,
            th.accent,
        ),
        Chip::new("Tab Switch", th.text, th.surface),
        Chip::new("↑↓ Navigate", th.text, th.surface),
    ];
    if app.zoom.is_grid() {
        chips.push(Chip::new("− More", th.text, th.surface));
        chips.push(Chip::new("+ Bigger", th.text, th.surface));
    }
    chips.extend([
        Chip::new("Enter Open", th.text, th.surface),
        Chip::new("Space Preview", th.text, th.surface),
        Chip::new("Mouse Drag", th.match_fg, th.select_bg),
        Chip::new("F4 Browse", th.text, th.surface),
        Chip::new("F8 Settings", th.text, th.surface),
    ]);
    let chips = fit_chips(chips, split[0].width);
    frame.render_widget(
        Paragraph::new(toolbar(&chips, split[0].width, th.bg)),
        split[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(status, Style::new().fg(th.dim).bg(th.bg)))
            .alignment(Alignment::Right)
            .style(Style::new().bg(th.bg)),
        split[1],
    );
}
