use qfind_core::CommandOutputExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use qfind_core::{Catalog, Manager};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::reactor::Sender;
use crate::theme::Theme;

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Projects,
    Storage,
    Git,
    Tasks,
}

impl Mode {
    fn title(self) -> &'static str {
        match self {
            Self::Projects => "Projects",
            Self::Storage => "Storage",
            Self::Git => "Git",
            Self::Tasks => "Tasks",
        }
    }

    fn component(self) -> &'static str {
        match self {
            Self::Projects => "projects",
            Self::Storage => "storage",
            Self::Git => "git",
            Self::Tasks => "tasks",
        }
    }
}

impl ProjectSort {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Repository => "repository",
            Self::Modified => "modified",
            Self::Artifacts => "artifacts",
        }
    }
}

#[derive(Clone)]
struct ProjectRow {
    path: PathBuf,
    repository: String,
    branch: String,
    rust: bool,
    node: bool,
    modified: i64,
    bytes: Option<u64>,
    artifacts: Vec<(PathBuf, Option<u64>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectSort {
    Name,
    Repository,
    Modified,
    Artifacts,
}

struct CleanupReview {
    paths: Vec<PathBuf>,
    selected: usize,
}

#[derive(Clone)]
struct StorageEntry {
    name: String,
    path: PathBuf,
    bytes: u64,
    is_dir: bool,
}

#[derive(Clone)]
struct StorageRoot {
    path: PathBuf,
    free: u64,
    total: u64,
}

#[derive(Default)]
struct StorageView {
    path: PathBuf,
    free: u64,
    total: u64,
    remaining: u64,
    entries: Vec<StorageEntry>,
    roots: Vec<StorageRoot>,
    global: bool,
}

#[derive(Default)]
struct GitView {
    root: PathBuf,
    status: String,
    files: Vec<String>,
    patch: String,
    patch_scroll: usize,
    staged: bool,
    unified: bool,
    wrap: bool,
    expanded: bool,
}

#[derive(Clone)]
struct TaskRow {
    id: String,
    title: String,
}

/// A typed, asynchronous workspace overlay for the shared shell components.
pub(super) struct Workspace {
    catalog: Catalog,
    path: PathBuf,
    events: Sender<crate::WorkEvent>,
    mode: Mode,
    selected: usize,
    scroll: usize,
    pending: Option<u64>,
    status: String,
    projects: Vec<ProjectRow>,
    project_order: Vec<usize>,
    project_filter: String,
    project_filtering: bool,
    project_sort: ProjectSort,
    cleanup: Option<CleanupReview>,
    storage: StorageView,
    git: GitView,
    tasks: Vec<TaskRow>,
    task_output: String,
    task_scroll: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Stay,
    Close,
    Browse(PathBuf),
    Copy(String),
}

impl Workspace {
    pub(super) fn new(
        catalog: Catalog,
        path: PathBuf,
        events: crate::reactor::Sender<crate::WorkEvent>,
    ) -> Self {
        let path = path.canonicalize().unwrap_or(path);
        let mut workspace = Self {
            catalog,
            storage: StorageView {
                path: path.clone(),
                ..StorageView::default()
            },
            path,
            events,
            mode: Mode::Projects,
            selected: 0,
            scroll: 0,
            pending: None,
            status: "Loading projects…".into(),
            projects: Vec::new(),
            project_order: Vec::new(),
            project_filter: String::new(),
            project_filtering: false,
            project_sort: ProjectSort::Name,
            cleanup: None,
            git: GitView::default(),
            tasks: Vec::new(),
            task_output: String::new(),
            task_scroll: 0,
        };
        workspace.refresh();
        workspace
    }

    pub(super) fn key(&mut self, key: KeyEvent) -> Outcome {
        if key.modifiers.intersects(
            crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
        ) {
            return Outcome::Stay;
        }
        if self.cleanup.is_some() {
            return self.key_cleanup(key);
        }
        if self.project_filtering {
            return self.key_project_filter(key);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Close,
            KeyCode::Char('1') => {
                self.switch_mode(Mode::Projects);
                Outcome::Stay
            }
            KeyCode::Char('2') => {
                self.switch_mode(Mode::Storage);
                Outcome::Stay
            }
            KeyCode::Char('3') => {
                self.switch_mode(Mode::Git);
                Outcome::Stay
            }
            KeyCode::Char('4') => {
                self.switch_mode(Mode::Tasks);
                Outcome::Stay
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.mode == Mode::Git && self.git.unified {
                    self.scroll_patch(-1);
                } else {
                    self.move_selection(-1);
                }
                Outcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.mode == Mode::Git && self.git.unified {
                    self.scroll_patch(1);
                } else {
                    self.move_selection(1);
                }
                Outcome::Stay
            }
            KeyCode::PageUp => {
                if self.mode == Mode::Tasks {
                    self.scroll_task(-8);
                } else {
                    self.scroll_patch(-8);
                }
                Outcome::Stay
            }
            KeyCode::PageDown => {
                if self.mode == Mode::Tasks {
                    self.scroll_task(8);
                } else {
                    self.scroll_patch(8);
                }
                Outcome::Stay
            }
            KeyCode::Home => {
                if self.mode == Mode::Git {
                    self.git.patch_scroll = 0;
                } else if self.mode == Mode::Tasks {
                    self.task_scroll = 0;
                } else {
                    self.selected = 0;
                    self.scroll = 0;
                }
                Outcome::Stay
            }
            KeyCode::End => {
                if self.mode == Mode::Git {
                    self.git.patch_scroll = self.git.patch.lines().count().saturating_sub(1);
                } else if self.mode == Mode::Tasks {
                    self.task_scroll = self.task_output.lines().count().saturating_sub(1);
                } else {
                    self.selected = self.row_count().saturating_sub(1);
                }
                Outcome::Stay
            }
            KeyCode::Char('[') if self.mode == Mode::Git => {
                self.move_hunk(-1);
                Outcome::Stay
            }
            KeyCode::Char(']') if self.mode == Mode::Git => {
                self.move_hunk(1);
                Outcome::Stay
            }
            KeyCode::Char('o') => self.browse_selected(),
            KeyCode::Enter => self.enter_selected(),
            KeyCode::Char('r') => {
                self.refresh_action(self.mode == Mode::Projects);
                Outcome::Stay
            }
            KeyCode::Char('/') if self.mode == Mode::Projects => {
                self.project_filtering = true;
                self.status = if self.project_filter.is_empty() {
                    "Filter projects: type a name or path".into()
                } else {
                    format!("Filter projects: {}", self.project_filter)
                };
                Outcome::Stay
            }
            KeyCode::Char('s') if self.mode == Mode::Projects => {
                self.project_sort = match self.project_sort {
                    ProjectSort::Name => ProjectSort::Repository,
                    ProjectSort::Repository => ProjectSort::Modified,
                    ProjectSort::Modified => ProjectSort::Artifacts,
                    ProjectSort::Artifacts => ProjectSort::Name,
                };
                self.rebuild_project_order();
                self.status = format!("Projects sorted by {}", self.project_sort.label());
                Outcome::Stay
            }
            KeyCode::Char('c') if self.mode == Mode::Projects => {
                self.open_cleanup();
                Outcome::Stay
            }
            KeyCode::Backspace if self.mode == Mode::Storage => {
                self.storage_back();
                Outcome::Stay
            }
            KeyCode::Char('g') if self.mode == Mode::Storage => {
                if self.pending.is_some() {
                    self.status = "wait for the current storage request".into();
                    return Outcome::Stay;
                }
                self.storage.global = true;
                self.selected = 0;
                self.scroll = 0;
                self.request(Mode::Storage, json!({"action":"roots", "roots":true}));
                Outcome::Stay
            }
            KeyCode::Tab if self.mode == Mode::Git => {
                if self.pending.is_none() {
                    self.git.staged = !self.git.staged;
                    self.refresh();
                }
                Outcome::Stay
            }
            KeyCode::Char('s') if self.mode == Mode::Git => {
                self.git_action("stage");
                Outcome::Stay
            }
            KeyCode::Char('u') if self.mode == Mode::Git => {
                self.git_action("unstage");
                Outcome::Stay
            }
            KeyCode::Char('d') if self.mode == Mode::Git => {
                self.git_action("diff");
                Outcome::Stay
            }
            KeyCode::Char('y') if self.mode == Mode::Git => {
                if self.git.patch.is_empty() {
                    self.status = "no Git patch to copy".into();
                    Outcome::Stay
                } else {
                    Outcome::Copy(self.git.patch.clone())
                }
            }
            KeyCode::Char('v') if self.mode == Mode::Git => {
                self.git.unified = !self.git.unified;
                self.status = if self.git.unified {
                    "Git diff: unified view"
                } else {
                    "Git diff: split view"
                }
                .into();
                Outcome::Stay
            }
            KeyCode::Char('w') if self.mode == Mode::Git => {
                self.git.wrap = !self.git.wrap;
                self.status = if self.git.wrap {
                    "Git diff: line wrapping on"
                } else {
                    "Git diff: line wrapping off"
                }
                .into();
                Outcome::Stay
            }
            KeyCode::Char('x') if self.mode == Mode::Git => {
                self.git.expanded = !self.git.expanded;
                self.status = if self.git.expanded {
                    "Git diff: expanded pane"
                } else {
                    "Git diff: standard pane"
                }
                .into();
                Outcome::Stay
            }
            _ => Outcome::Stay,
        }
    }

    pub(super) fn apply(&mut self, id: u64, result: Result<Value, String>) {
        if self.pending != Some(id) {
            return;
        }
        self.pending = None;
        match result {
            Err(error) if self.mode == Mode::Tasks => {
                self.task_output = error;
                self.task_scroll = 0;
                self.status = self
                    .task_output
                    .lines()
                    .next()
                    .unwrap_or("Task failed")
                    .to_owned();
            }
            Err(error) => self.status = format!("{} failed: {error}", self.mode.title()),
            Ok(value) => match self.mode {
                Mode::Projects => self.apply_projects(value),
                Mode::Storage => self.apply_storage(value),
                Mode::Git => self.apply_git(value),
                Mode::Tasks => self.apply_tasks(value),
            },
        }
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
        let popup = centered(area, 112, 34);
        frame.render_widget(Clear, popup);
        let title = format!(
            "Workspace · {}{}",
            self.mode.title(),
            busy_suffix(self.pending)
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.accent))
            .style(Style::new().bg(theme.surface));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.width < 8 || inner.height < 5 {
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(inner);
        draw_tabs(frame, rows[0], self.mode, theme);
        match self.mode {
            Mode::Git => self.draw_git(frame, rows[1], theme),
            Mode::Projects => self.draw_projects(frame, rows[1], theme),
            Mode::Storage => self.draw_storage(frame, rows[1], theme),
            Mode::Tasks => self.draw_tasks(frame, rows[1], theme),
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                self.status.clone(),
                Style::new().fg(theme.match_fg),
            )))
            .style(Style::new().bg(theme.surface)),
            rows[2],
        );
        frame.render_widget(
            Paragraph::new(help_line(self.mode, theme)).style(Style::new().bg(theme.surface)),
            rows[3],
        );
    }

    fn switch_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        if self.pending.is_some() {
            self.status = "wait for the current workspace request".into();
            return;
        }
        if self.mode == Mode::Projects {
            if let Some(path) = self.selected_path() {
                self.path = path;
            }
        }
        self.mode = mode;
        self.selected = 0;
        self.scroll = 0;
        if mode == Mode::Storage {
            self.storage.global = false;
            self.storage.path = self.path.clone();
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        self.refresh_action(false);
    }

    fn refresh_action(&mut self, refresh_projects: bool) {
        if self.pending.is_some() {
            self.status = "workspace request already running".into();
            return;
        }
        let request = match self.mode {
            Mode::Projects if refresh_projects => json!({"action":"refresh"}),
            Mode::Projects => json!({"action":"list"}),
            Mode::Storage if self.storage.global => {
                json!({"action":"roots", "roots":true})
            }
            Mode::Storage => json!({"action":"map", "path":self.storage.path}),
            Mode::Git => json!({
                "action":"status",
                "path":self.path,
                "staged":self.git.staged,
            }),
            Mode::Tasks => json!({"action":"list", "path":self.path}),
        };
        self.request(self.mode, request);
    }

    fn key_project_filter(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                self.project_filtering = false;
                self.status = format!("Projects sorted by {}", self.project_sort.label());
            }
            KeyCode::Enter => {
                self.project_filtering = false;
                self.selected = 0;
                self.scroll = 0;
                self.rebuild_project_order();
                self.status = format!("{} projects match", self.project_order.len());
            }
            KeyCode::Backspace => {
                self.project_filter.pop();
                self.rebuild_project_order();
                self.selected = self
                    .selected
                    .min(self.project_order.len().saturating_sub(1));
                self.status = format!("Filter projects: {}", self.project_filter);
            }
            KeyCode::Char(character) => {
                self.project_filter.push(character);
                self.rebuild_project_order();
                self.selected = self
                    .selected
                    .min(self.project_order.len().saturating_sub(1));
                self.status = format!("Filter projects: {}", self.project_filter);
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn key_cleanup(&mut self, key: KeyEvent) -> Outcome {
        let Some(review) = self.cleanup.as_mut() else {
            return Outcome::Stay;
        };
        match key.code {
            KeyCode::Esc => {
                self.cleanup = None;
                self.status = "Cleanup review canceled".into();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                review.selected = review.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                review.selected = (review.selected + 1).min(review.paths.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                let paths = review.paths.clone();
                self.cleanup = None;
                self.request(Mode::Projects, json!({"action":"cleanup", "paths":paths}));
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn open_cleanup(&mut self) {
        let Some(project) = self.selected_project() else {
            self.status = "select a project first".into();
            return;
        };
        if project.artifacts.is_empty() {
            self.status = "selected project has no local artifacts".into();
            return;
        }
        self.cleanup = Some(CleanupReview {
            paths: project
                .artifacts
                .iter()
                .map(|(path, _)| path.clone())
                .collect(),
            selected: 0,
        });
        self.status = "Review cleanup paths; Enter moves them to Qfind Trash".into();
    }

    fn storage_back(&mut self) {
        if self.pending.is_some() {
            self.status = "wait for the current storage request".into();
            return;
        }
        if self.storage.global {
            self.storage.global = false;
            self.storage.path = self.path.clone();
            self.selected = 0;
            self.scroll = 0;
            self.refresh();
            return;
        }
        let Some(parent) = self.storage.path.parent().map(PathBuf::from) else {
            return;
        };
        if parent == self.storage.path {
            return;
        }
        self.storage.path = parent;
        self.selected = 0;
        self.scroll = 0;
        self.refresh();
    }

    fn selected_project(&self) -> Option<&ProjectRow> {
        self.project_order
            .get(self.selected)
            .and_then(|index| self.projects.get(*index))
    }

    fn rebuild_project_order(&mut self) {
        let query = self.project_filter.to_lowercase();
        let mut order = self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, project)| {
                query.is_empty()
                    || project
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query)
                    || project.repository.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        order.sort_by(|a, b| {
            let left = &self.projects[*a];
            let right = &self.projects[*b];
            let order = match self.project_sort {
                ProjectSort::Name => path_name(&left.path)
                    .to_lowercase()
                    .cmp(&path_name(&right.path).to_lowercase()),
                ProjectSort::Repository => left
                    .repository
                    .to_lowercase()
                    .cmp(&right.repository.to_lowercase()),
                ProjectSort::Modified => right.modified.cmp(&left.modified),
                ProjectSort::Artifacts => right.artifacts.len().cmp(&left.artifacts.len()),
            };
            order.then_with(|| left.path.cmp(&right.path))
        });
        self.project_order = order;
    }

    fn request(&mut self, mode: Mode, request: Value) {
        if self.pending.is_some() {
            self.status = "workspace request already running".into();
            return;
        }
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        self.pending = Some(id);
        self.status = format!("Loading {}…", mode.title());
        let events = self.events.clone();
        let catalog = self.catalog.clone();
        let path = self.path.clone();
        thread::spawn(move || {
            let result = dispatch(mode, catalog, path, request);
            let _ = events.send(crate::WorkEvent::Workspace(id, result));
        });
    }

    fn move_selection(&mut self, direction: i8) {
        let count = self.row_count();
        if count == 0 {
            return;
        }
        self.selected = if direction < 0 {
            self.selected.saturating_sub(1)
        } else {
            (self.selected + 1).min(count - 1)
        };
    }

    fn browse_selected(&mut self) -> Outcome {
        let Some(mut path) = self.selected_path() else {
            return Outcome::Stay;
        };
        if !path.is_dir() {
            let Some(parent) = path.parent().map(PathBuf::from) else {
                return Outcome::Stay;
            };
            path = parent;
        }
        self.path = path.clone();
        Outcome::Browse(path)
    }

    fn row_count(&self) -> usize {
        match self.mode {
            Mode::Projects => self.project_order.len(),
            Mode::Storage => {
                if self.storage.global {
                    self.storage.roots.len()
                } else {
                    self.storage.entries.len()
                }
            }
            Mode::Git => self.git.files.len(),
            Mode::Tasks => self.tasks.len(),
        }
    }

    fn selected_path(&self) -> Option<PathBuf> {
        match self.mode {
            Mode::Projects => self.selected_project().map(|row| row.path.clone()),
            Mode::Storage if self.storage.global => self
                .storage
                .roots
                .get(self.selected)
                .map(|row| row.path.clone()),
            Mode::Storage => self
                .storage
                .entries
                .get(self.selected)
                .map(|row| row.path.clone()),
            Mode::Git => self
                .git
                .files
                .get(self.selected)
                .map(|file| self.git.root.join(file)),
            Mode::Tasks => Some(self.path.clone()),
        }
    }

    fn enter_selected(&mut self) -> Outcome {
        match self.mode {
            Mode::Projects => {
                let Some(path) = self.selected_path() else {
                    return Outcome::Stay;
                };
                self.path = path.clone();
                Outcome::Browse(path)
            }
            Mode::Storage if self.storage.global => {
                if self.pending.is_some() {
                    self.status = "wait for the current storage request".into();
                    return Outcome::Stay;
                }
                if let Some(path) = self.selected_path() {
                    self.storage.global = false;
                    self.storage.path = path;
                    self.selected = 0;
                    self.scroll = 0;
                    self.refresh();
                }
                Outcome::Stay
            }
            Mode::Storage => {
                if self.pending.is_some() {
                    self.status = "wait for the current storage request".into();
                    return Outcome::Stay;
                }
                if let Some(path) = self.selected_path().filter(|path| path.is_dir()) {
                    self.storage.path = path;
                    self.selected = 0;
                    self.scroll = 0;
                    self.refresh();
                }
                Outcome::Stay
            }
            Mode::Git => {
                self.git_action("diff");
                Outcome::Stay
            }
            Mode::Tasks => {
                let Some(task) = self.tasks.get(self.selected).cloned() else {
                    return Outcome::Stay;
                };
                self.request(
                    Mode::Tasks,
                    json!({"action":"run", "command":task.id, "path":self.path}),
                );
                Outcome::Stay
            }
        }
    }

    fn git_action(&mut self, action: &'static str) {
        let Some(file) = self.git.files.get(self.selected).cloned() else {
            self.status = "select a changed file first".into();
            return;
        };
        self.request(
            Mode::Git,
            json!({
                "action":action,
                "path":self.path,
                "file":file,
                "staged":self.git.staged,
            }),
        );
    }

    fn scroll_patch(&mut self, delta: i32) {
        if self.mode != Mode::Git {
            return;
        }
        let max = self.git.patch.lines().count().saturating_sub(1);
        self.git.patch_scroll = if delta.is_negative() {
            self.git
                .patch_scroll
                .saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (self.git.patch_scroll + delta as usize).min(max)
        };
    }

    fn scroll_task(&mut self, delta: i32) {
        if self.mode != Mode::Tasks {
            return;
        }
        let max = self.task_output.lines().count().saturating_sub(1);
        self.task_scroll = if delta.is_negative() {
            self.task_scroll
                .saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (self.task_scroll + delta as usize).min(max)
        };
    }

    fn move_hunk(&mut self, direction: i8) {
        let starts = self
            .git
            .patch
            .lines()
            .enumerate()
            .filter_map(|(index, line)| line.starts_with("@@ ").then_some(index))
            .collect::<Vec<_>>();
        if starts.is_empty() {
            return;
        }
        self.git.patch_scroll = if direction < 0 {
            starts
                .iter()
                .rev()
                .find(|&&start| start < self.git.patch_scroll)
                .copied()
                .unwrap_or(starts[0])
        } else {
            starts
                .iter()
                .find(|&&start| start > self.git.patch_scroll)
                .copied()
                .unwrap_or(*starts.last().unwrap_or(&0))
        };
    }

    fn apply_projects(&mut self, value: Value) {
        if value["cleanup"].as_bool().unwrap_or(false) {
            self.status = value["text"]
                .as_str()
                .unwrap_or("Cleanup finished")
                .to_owned();
            self.refresh();
            return;
        }
        let Some(rows) = value["projects"].as_array() else {
            self.status = "Projects returned an invalid response".into();
            return;
        };
        self.projects = rows.iter().filter_map(project_row).collect();
        self.rebuild_project_order();
        self.selected = self
            .selected
            .min(self.project_order.len().saturating_sub(1));
        self.status = if self.project_filter.is_empty() {
            format!("{} projects", self.project_order.len())
        } else {
            format!(
                "{} of {} projects match `{}`",
                self.project_order.len(),
                self.projects.len(),
                self.project_filter
            )
        };
    }

    fn apply_storage(&mut self, value: Value) {
        if let Some(roots) = value["roots"].as_array() {
            self.storage.roots = roots.iter().filter_map(storage_root).collect();
            self.storage.entries.clear();
            self.selected = self
                .selected
                .min(self.storage.roots.len().saturating_sub(1));
            self.status = format!("{} storage roots", self.storage.roots.len());
            return;
        }
        self.storage.path = path_value(&value["path"]).unwrap_or_else(|| self.storage.path.clone());
        self.storage.free = value["free"].as_u64().unwrap_or(0);
        self.storage.total = value["total"].as_u64().unwrap_or(0);
        self.storage.remaining = value["remaining"].as_u64().unwrap_or(0);
        self.storage.entries = value["entries"]
            .as_array()
            .map(|entries| entries.iter().filter_map(storage_entry).collect())
            .unwrap_or_default();
        self.selected = self
            .selected
            .min(self.storage.entries.len().saturating_sub(1));
        self.status = format!(
            "{} entries · {} free of {}",
            self.storage.entries.len(),
            human_size(self.storage.free),
            human_size(self.storage.total)
        );
    }

    fn apply_git(&mut self, value: Value) {
        if let Some(root) = path_value(&value["root"]) {
            self.git.root = root;
        }
        if let Some(status) = value["status"].as_str() {
            self.git.status = status.to_owned();
        }
        if let Some(files) = value["files"].as_array() {
            self.git.files = files
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            self.selected = self.selected.min(self.git.files.len().saturating_sub(1));
        }
        if let Some(text) = value["text"].as_str() {
            self.git.patch = text.to_owned();
            self.git.patch_scroll = 0;
        }
        self.status = value["text"]
            .as_str()
            .filter(|text| !text.is_empty())
            .map_or_else(
                || format!("{} changed files", self.git.files.len()),
                str::to_owned,
            );
        if value["files"].is_null() && value["text"].as_str().is_some() {
            self.refresh();
        }
    }

    fn apply_tasks(&mut self, value: Value) {
        if let Some(commands) = value["commands"].as_array() {
            self.tasks = commands.iter().filter_map(task_row).collect();
            self.task_output.clear();
            self.task_scroll = 0;
            self.selected = self.selected.min(self.tasks.len().saturating_sub(1));
            self.status = format!("{} task commands", self.tasks.len());
        } else if let Some(text) = value["text"].as_str() {
            self.task_output = text.to_owned();
            self.task_scroll = 0;
            self.status = text.lines().next().unwrap_or(text).to_owned();
        } else {
            self.status = "Tasks returned an invalid response".into();
        }
    }

    fn draw_projects(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
        let chunks = split_body(area);
        let lines = self
            .project_order
            .iter()
            .filter_map(|index| self.projects.get(*index))
            .map(|row| {
                let marker = if row.repository.is_empty() {
                    "◇"
                } else {
                    "◆"
                };
                let repository = row.repository.rsplit('/').next().unwrap_or("local");
                Line::from(format!(
                    "{marker} {repository}  {}  {}",
                    path_name(&row.path),
                    row.branch
                ))
            })
            .collect::<Vec<_>>();
        draw_rows(
            frame,
            chunks[0],
            &lines,
            &mut self.selected,
            &mut self.scroll,
            theme,
        );
        if let Some(review) = &self.cleanup {
            draw_detail(
                frame,
                chunks[1],
                "Cleanup review",
                cleanup_detail(review),
                theme,
            );
        } else {
            let detail = self
                .selected_project()
                .map(project_detail)
                .unwrap_or_else(|| vec![Line::from("No projects found")]);
            draw_detail(frame, chunks[1], "Project", detail, theme);
        }
    }

    fn draw_storage(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
        let chunks = split_body(area);
        let lines = if self.storage.global {
            self.storage
                .roots
                .iter()
                .map(|root| Line::from(format!("▣ {}", root.path.display())))
                .collect::<Vec<_>>()
        } else {
            self.storage
                .entries
                .iter()
                .map(|entry| {
                    Line::from(format!(
                        "{} {:>10}  {}",
                        if entry.is_dir { "▸" } else { "·" },
                        human_size(entry.bytes),
                        entry.name
                    ))
                })
                .collect::<Vec<_>>()
        };
        draw_rows(
            frame,
            chunks[0],
            &lines,
            &mut self.selected,
            &mut self.scroll,
            theme,
        );
        let mut detail = vec![Line::from(if self.storage.global {
            "Mounted roots"
        } else {
            "Directory"
        })];
        let detail_path = self
            .storage
            .global
            .then(|| {
                self.storage
                    .roots
                    .get(self.selected)
                    .map(|root| root.path.clone())
            })
            .flatten()
            .unwrap_or_else(|| self.storage.path.clone());
        detail.push(Line::from(detail_path.display().to_string()));
        if self.storage.global {
            if let Some(root) = self.storage.roots.get(self.selected) {
                detail.push(Line::from(format!("Free: {}", human_size(root.free))));
                detail.push(Line::from(format!("Total: {}", human_size(root.total))));
            }
        } else {
            detail.push(Line::from(format!(
                "Free: {}",
                human_size(self.storage.free)
            )));
            detail.push(Line::from(format!(
                "Total: {}",
                human_size(self.storage.total)
            )));
            if self.storage.remaining > 0 {
                detail.push(Line::from(format!(
                    "Other entries: {}",
                    human_size(self.storage.remaining)
                )));
            }
        }
        draw_detail(frame, chunks[1], "Storage", detail, theme);
    }

    fn draw_git(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
        let patch = if self.git.patch.is_empty() {
            self.git.status.clone()
        } else {
            self.git.patch.clone()
        };
        let title = match (self.git.staged, self.git.unified) {
            (true, true) => "Diff · staged · unified",
            (true, false) => "Diff · staged",
            (false, true) => "Diff · working tree · unified",
            (false, false) => "Diff · working tree",
        };
        if self.git.unified {
            draw_patch(
                frame,
                area,
                &patch,
                title,
                self.git.patch_scroll,
                self.git.wrap,
                theme,
            );
            return;
        }
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(if self.git.expanded { 20 } else { 35 }),
                Constraint::Min(1),
            ])
            .split(area);
        let lines = self
            .git
            .files
            .iter()
            .map(|file| Line::from(file.clone()))
            .collect::<Vec<_>>();
        draw_rows(
            frame,
            chunks[0],
            &lines,
            &mut self.selected,
            &mut self.scroll,
            theme,
        );
        draw_patch(
            frame,
            chunks[1],
            &patch,
            title,
            self.git.patch_scroll,
            self.git.wrap,
            theme,
        );
    }

    fn draw_tasks(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
        let chunks = split_body(area);
        let lines = self
            .tasks
            .iter()
            .map(|task| Line::from(format!("{}  {}", task.id, task.title)))
            .collect::<Vec<_>>();
        draw_rows(
            frame,
            chunks[0],
            &lines,
            &mut self.selected,
            &mut self.scroll,
            theme,
        );
        let mut detail = self
            .tasks
            .get(self.selected)
            .map(|task| {
                vec![
                    Line::from(task.title.clone()),
                    Line::from(format!("Command: {}", task.id)),
                    Line::from(format!("In: {}", self.path.display())),
                    Line::from("Enter runs this command"),
                ]
            })
            .unwrap_or_else(|| vec![Line::from("No task commands found")]);
        if !self.task_output.is_empty() {
            detail.push(Line::from(""));
            detail.push(Line::from("Output:"));
            detail.extend(
                self.task_output
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
        }
        draw_detail_scrolled(frame, chunks[1], "Task", detail, self.task_scroll, theme);
    }
}

fn dispatch(mode: Mode, catalog: Catalog, path: PathBuf, request: Value) -> Result<Value, String> {
    if mode == Mode::Projects && request["action"] == "cleanup" {
        return cleanup(request["paths"].as_array().ok_or("No cleanup paths")?);
    }
    if mode == Mode::Storage && request["roots"].as_bool().unwrap_or(false) {
        let mut roots = Vec::new();
        for root in qfind_core::discover_mounts() {
            let (free, total) = qfind_core::components::capacity(&root)?;
            roots.push(json!({
                "path":root,
                "free":free,
                "total":total,
            }));
        }
        return Ok(json!({"roots":roots}));
    }
    let manager = if matches!(mode, Mode::Git | Mode::Tasks) {
        Manager::live(Some(path.clone()))
    } else {
        Manager::new(catalog, Some(path))
    };
    dispatch_component(&manager, mode.component(), request)
}

fn cleanup(paths: &[Value]) -> Result<Value, String> {
    if paths.is_empty() {
        return Err("No cleanup paths".into());
    }
    let paths = paths
        .iter()
        .map(|value| {
            let path = path_value(value).ok_or("Invalid cleanup path")?;
            if !path.is_dir() || path.canonicalize().ok().as_deref() != Some(path.as_path()) {
                return Err(format!("Path changed since review: {}", path.display()));
            }
            Ok(path)
        })
        .collect::<Result<Vec<_>, String>>()?;
    if builds_active()? {
        return Err("Cleanup is blocked while a build or package process is active".into());
    }
    for path in &paths {
        if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
            let tracked = Command::new("git")
                .arg("-C")
                .arg(parent)
                .args(["ls-files", "-z", "--"])
                .arg(name)
                .bounded_output(Duration::from_secs(20))
                .map_err(|error| format!("Cannot check tracked files: {error}"))?;
            if !tracked.status.success() {
                return Err(format!(
                    "Cannot verify Git-tracked files in {}",
                    path.display()
                ));
            }
            if tracked.status.success() && !tracked.stdout.is_empty() {
                return Err(format!(
                    "Cleanup blocked: {} contains Git-tracked files",
                    path.display()
                ));
            }
        }
    }
    let mut done = 0;
    for path in paths {
        if builds_active()? {
            return Err(format!(
                "Moved {done} folders; cleanup blocked by an active build/package process"
            ));
        }
        qfind_core::trash(&path)
            .map_err(|error| format!("Moved {done} folders; {path:?}: {error}"))?;
        done += 1;
    }
    Ok(json!({
        "cleanup":true,
        "text":format!("Moved {done} folders to Qfind Trash"),
    }))
}

#[cfg(target_os = "linux")]
fn builds_active() -> Result<bool, String> {
    for entry in std::fs::read_dir("/proc").map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        match std::fs::read_to_string(entry.path().join("comm")) {
            Ok(name)
                if matches!(
                    name.trim(),
                    "cargo" | "rustc" | "rust-analyzer" | "npm" | "bun" | "node" | "pnpm" | "yarn"
                ) =>
            {
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err("Cannot inspect active processes; cleanup is unavailable".into());
            }
            _ => {}
        }
    }
    Ok(false)
}

#[cfg(not(target_os = "linux"))]
fn builds_active() -> Result<bool, String> {
    #[cfg(windows)]
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .bounded_output(Duration::from_secs(10));
    #[cfg(not(windows))]
    let output = Command::new("ps")
        .args(["-A", "-o", "comm="])
        .bounded_output(Duration::from_secs(10));
    let output = output.map_err(|error| format!("Cannot inspect active processes: {error}"))?;
    if !output.status.success() {
        return Err("Cannot inspect active processes; cleanup is unavailable".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let command = if cfg!(windows) {
            line.trim_start_matches('"').split('"').next().unwrap_or("")
        } else {
            line.trim()
        };
        let name = std::path::Path::new(command)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        matches!(
            name,
            "cargo" | "rustc" | "rust-analyzer" | "npm" | "bun" | "node" | "pnpm" | "yarn"
        )
    }))
}

fn dispatch_component(manager: &Manager, component: &str, request: Value) -> Result<Value, String> {
    let text = qfind_core::components::dispatch(manager, component, &request.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn project_row(value: &Value) -> Option<ProjectRow> {
    Some(ProjectRow {
        path: path_value(&value["path"])?,
        repository: value["repository"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                value["git"]
                    .as_bool()
                    .filter(|git| *git)
                    .map(|_| "Git".into())
            })
            .unwrap_or_default(),
        branch: value["branch"].as_str().unwrap_or_default().to_owned(),
        rust: value["rust"].as_bool().unwrap_or(false),
        node: value["node"].as_bool().unwrap_or(false),
        modified: value["modified"]
            .as_i64()
            .or_else(|| value["modified"].as_bool().map(i64::from))
            .unwrap_or(0),
        bytes: value["bytes"].as_u64(),
        artifacts: value["artifacts"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Some((path_value(&item["path"])?, item["bytes"].as_u64())))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn storage_entry(value: &Value) -> Option<StorageEntry> {
    Some(StorageEntry {
        name: value["name"].as_str()?.to_owned(),
        path: path_value(&value["path"])?,
        bytes: value["bytes"].as_u64().unwrap_or(0),
        is_dir: value["is_dir"].as_bool().unwrap_or(false),
    })
}

fn storage_root(value: &Value) -> Option<StorageRoot> {
    Some(StorageRoot {
        path: path_value(&value["path"])?,
        free: value["free"].as_u64().unwrap_or(0),
        total: value["total"].as_u64().unwrap_or(0),
    })
}

fn task_row(value: &Value) -> Option<TaskRow> {
    Some(TaskRow {
        id: value["id"].as_str()?.to_owned(),
        title: value["title"].as_str()?.to_owned(),
    })
}

fn path_value(value: &Value) -> Option<PathBuf> {
    value.as_str().map(PathBuf::from)
}

fn project_detail(row: &ProjectRow) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(row.path.display().to_string())];
    lines.push(Line::from(if row.repository.is_empty() {
        "Git: no repository".into()
    } else {
        format!("Git: {} · {}", row.repository, row.branch)
    }));
    let mut tools = Vec::new();
    if row.rust {
        tools.push("Rust");
    }
    if row.node {
        tools.push("Node");
    }
    if !tools.is_empty() {
        lines.push(Line::from(format!("Tools: {}", tools.join(" · "))));
    }
    if row.modified > 0 {
        lines.push(Line::from(format!(
            "Modified: {}",
            human_mtime(row.modified)
        )));
    }
    if let Some(bytes) = row.bytes {
        lines.push(Line::from(format!("Size: {}", human_size(bytes))));
    }
    if !row.artifacts.is_empty() {
        lines.push(Line::from(format!("Artifacts: {}", row.artifacts.len())));
        for (path, bytes) in row.artifacts.iter().take(6) {
            lines.push(Line::from(format!(
                "  {}{}",
                path_name(path),
                bytes
                    .map(|size| format!(" · {}", human_size(size)))
                    .unwrap_or_default()
            )));
        }
    }
    lines
}

fn cleanup_detail(review: &CleanupReview) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("Review selected build and dependency folders"),
        Line::from("Enter moves every reviewed folder to Qfind Trash"),
        Line::from("Esc cancels; cleanup is blocked for changed paths"),
        Line::from(""),
    ];
    for (index, path) in review.paths.iter().enumerate() {
        lines.push(Line::from(format!(
            "{} {}",
            if index == review.selected { ">" } else { " " },
            path.display()
        )));
    }
    lines
}

fn draw_tabs(frame: &mut Frame, area: Rect, selected: Mode, theme: Theme) {
    let tabs = [
        (Mode::Projects, "1 Projects"),
        (Mode::Storage, "2 Storage"),
        (Mode::Git, "3 Git"),
        (Mode::Tasks, "4 Tasks"),
    ];
    let mut spans = Vec::new();
    for (index, (mode, label)) in tabs.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            label,
            Style::new()
                .fg(if mode == selected {
                    theme.accent
                } else {
                    theme.dim
                })
                .add_modifier(if mode == selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_rows(
    frame: &mut Frame,
    area: Rect,
    lines: &[Line<'static>],
    selected: &mut usize,
    scroll: &mut usize,
    theme: Theme,
) {
    let block = Block::default()
        .title("Items")
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = inner.height as usize;
    if lines.is_empty() {
        frame.render_widget(
            Paragraph::new("No items").style(Style::new().fg(theme.dim)),
            inner,
        );
        return;
    }
    *selected = (*selected).min(lines.len() - 1);
    if *selected < *scroll {
        *scroll = *selected;
    } else if *selected >= *scroll + height.max(1) {
        *scroll = selected.saturating_add(1).saturating_sub(height.max(1));
    }
    let visible = lines
        .iter()
        .enumerate()
        .skip(*scroll)
        .take(height.max(1))
        .map(|(index, line)| {
            if index == *selected {
                line.clone()
                    .style(Style::new().fg(theme.text).bg(theme.select_bg))
            } else {
                line.clone().style(Style::new().fg(theme.text))
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn draw_detail(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    theme: Theme,
) {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(theme.text))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_detail_scrolled(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    scroll: usize,
    theme: Theme,
) {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(theme.text))
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

fn draw_patch(
    frame: &mut Frame,
    area: Rect,
    patch: &str,
    title: &str,
    scroll: usize,
    wrap: bool,
    theme: Theme,
) {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = patch
        .lines()
        .map(|line| {
            let color = if line.starts_with("+++") || line.starts_with("---") {
                theme.dim
            } else if line.starts_with('+') {
                theme.sky
            } else if line.starts_with('-') {
                theme.pink
            } else if line.starts_with("@@") {
                theme.accent
            } else if line.starts_with("diff ") || line.starts_with("index ") {
                theme.purple
            } else {
                theme.text
            };
            Line::from(Span::styled(line.to_owned(), Style::new().fg(color)))
        })
        .collect::<Vec<_>>();
    let mut paragraph = Paragraph::new(lines)
        .style(Style::new().fg(theme.text).bg(theme.surface))
        .scroll((scroll.min(u16::MAX as usize) as u16, 0));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, inner);
}

fn split_body(area: Rect) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Min(1)])
        .split(area);
    [chunks[0], chunks[1]]
}

fn centered(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = max_width.min(area.width.saturating_sub(2)).max(2);
    let height = max_height.min(area.height.saturating_sub(2)).max(2);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn busy_suffix(pending: Option<u64>) -> &'static str {
    if pending.is_some() { " · loading" } else { "" }
}

fn help_line(mode: Mode, theme: Theme) -> Line<'static> {
    let text = match mode {
        Mode::Projects => {
            "↑↓ select · Enter/o browse · / filter · s sort · c cleanup · r refresh · Esc close"
        }
        Mode::Storage => {
            "↑↓ select · Enter drill · Backspace parent · g roots · o browse · r refresh · Esc close"
        }
        Mode::Git => {
            "↑↓ file · d diff · s/u stage · Tab staged · v unified · w wrap · x expand · [/] hunk · y copy"
        }
        Mode::Tasks => {
            "↑↓ select · Enter run · PgUp/Dn output · r refresh · 1–4 switch · Esc close"
        }
    };
    Line::from(Span::styled(text, Style::new().fg(theme.dim)))
}

fn path_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_str().unwrap_or("/"))
        .to_owned()
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

fn human_mtime(secs: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(secs);
    let delta = now.saturating_sub(secs);
    if delta < 45 {
        "just now".into()
    } else if delta < 90 {
        "1 min ago".into()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 36 * 3600 {
        format!("{} h ago", (delta + 1800) / 3600)
    } else if delta < 14 * 86400 {
        format!("{} days ago", (delta + 43200) / 86400)
    } else {
        format!("{} days ago", delta / 86400)
    }
}
