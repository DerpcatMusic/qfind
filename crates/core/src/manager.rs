use std::path::{Path, PathBuf};

use crate::nav::Location;
use crate::plugin::PluginHost;
use crate::{Catalog, Error, Result, SearchOpts, StorageMap};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowseMode {
    #[default]
    Classic,
    Qfind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocationScope {
    #[default]
    Directory,
    Global,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartScope {
    #[default]
    Directory,
    Global,
}

/// Shared navigation and view state for graphical and terminal managers.
#[derive(Clone, Debug, Default)]
pub struct ManagerSession {
    directory: Option<PathBuf>,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    selected: Option<PathBuf>,
    mode: BrowseMode,
    search_scope: LocationScope,
    chart_scope: ChartScope,
}

impl ManagerSession {
    #[must_use]
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            search_scope: if directory.is_some() {
                LocationScope::Directory
            } else {
                LocationScope::Global
            },
            directory,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    #[must_use]
    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    #[must_use]
    pub fn mode(&self) -> BrowseMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: BrowseMode) {
        self.mode = mode;
    }

    #[must_use]
    pub fn search_scope(&self) -> LocationScope {
        self.search_scope
    }

    pub fn set_search_scope(&mut self, scope: LocationScope) {
        self.search_scope = scope;
    }

    #[must_use]
    pub fn chart_scope(&self) -> ChartScope {
        self.chart_scope
    }

    pub fn set_chart_scope(&mut self, scope: ChartScope) {
        self.chart_scope = scope;
    }

    pub fn select(&mut self, path: Option<PathBuf>) {
        self.selected = path;
    }

    pub fn navigate(&mut self, path: PathBuf) -> bool {
        if self.directory.as_ref() == Some(&path) {
            self.search_scope = LocationScope::Directory;
            return false;
        }
        if let Some(current) = self.directory.replace(path) {
            self.back.push(current);
        }
        self.forward.clear();
        self.search_scope = LocationScope::Directory;
        self.selected = None;
        true
    }

    pub fn back(&mut self) -> Option<PathBuf> {
        let path = self.back.pop()?;
        if let Some(current) = self.directory.replace(path.clone()) {
            self.forward.push(current);
        }
        self.search_scope = LocationScope::Directory;
        self.selected = None;
        Some(path)
    }

    pub fn forward(&mut self) -> Option<PathBuf> {
        let path = self.forward.pop()?;
        if let Some(current) = self.directory.replace(path.clone()) {
            self.back.push(current);
        }
        self.search_scope = LocationScope::Directory;
        self.selected = None;
        Some(path)
    }

    #[must_use]
    pub fn parent(&self) -> Option<PathBuf> {
        self.directory()?.parent().map(Path::to_path_buf)
    }

    #[must_use]
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    #[must_use]
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Current position as a shared [`Location`] value for breadcrumbs.
    #[must_use]
    pub fn location(&self) -> Location {
        match &self.directory {
            None => Location::Global,
            Some(path) => Location::Directory(path.clone()),
        }
    }

    /// Jump to a [`Location`]. Directory jumps push history like
    /// [`navigate`](Self::navigate); going Global keeps history so Back can
    /// return to the last directory.
    pub fn set_location(&mut self, location: Location) {
        match location {
            Location::Global => {
                self.directory = None;
                self.search_scope = LocationScope::Global;
                self.selected = None;
            }
            Location::Directory(path) => {
                self.navigate(path);
            }
        }
    }

    /// Pure state transition behind [`Manager::dispatch`]. Unvalidated:
    /// `Manager::dispatch` rejects unknown directories first.
    pub fn dispatch(&mut self, action: Action) -> Outcome {
        match action {
            Action::Navigate(path) => Outcome::Navigated(self.navigate(path)),
            Action::Back => Outcome::Moved(self.back()),
            Action::Forward => Outcome::Moved(self.forward()),
            Action::Select(path) => {
                self.select(path);
                Outcome::Selected
            }
            Action::Locate(location) => {
                self.set_location(location);
                Outcome::Located
            }
            Action::SearchScope(scope) => {
                self.set_search_scope(scope);
                Outcome::ScopeChanged
            }
            Action::ChartScope(scope) => {
                self.set_chart_scope(scope);
                Outcome::ScopeChanged
            }
            Action::BrowseMode(mode) => {
                self.set_mode(mode);
                Outcome::ScopeChanged
            }
            // Plugin actions never reach session state; `Manager::dispatch`
            // routes them to `PluginHost` before delegating here.
            Action::Plugin { .. } => Outcome::PluginHandled(false),
        }
    }
}

/// Every state change a frontend can request. GTK, TUI, CLI, the native FFI,
/// and plugins all funnel through [`Manager::dispatch`] — the single dispatch
/// point. Reads (`view`, `chart`) stay plain methods; they change no state.
#[derive(Clone, Debug)]
pub enum Action {
    Navigate(PathBuf),
    Back,
    Forward,
    Select(Option<PathBuf>),
    Locate(Location),
    SearchScope(LocationScope),
    ChartScope(ChartScope),
    BrowseMode(BrowseMode),
    Plugin { name: String, arg: String },
}

/// What [`Manager::dispatch`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Navigated(bool),
    Moved(Option<PathBuf>),
    Selected,
    Located,
    ScopeChanged,
    PluginHandled(bool),
}

#[derive(Clone, Debug)]
pub struct ManagerRow {
    pub id: Option<u32>,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub bytes: u64,
    pub entries: u64,
}

#[derive(Clone, Debug)]
pub struct ManagerView {
    pub directory: Option<PathBuf>,
    pub rows: Vec<ManagerRow>,
    pub folders: usize,
    pub files: usize,
}

/// Shared file-manager behavior. Platform adapters render its owned snapshots.
#[derive(Clone)]
pub struct Manager {
    catalog: Option<Catalog>,
    storage: Option<std::sync::Arc<StorageMap>>,
    session: ManagerSession,
}

impl Manager {
    pub(crate) fn catalog(&self) -> Option<&Catalog> { self.catalog.as_ref() }
    pub(crate) fn storage(&self) -> Option<&StorageMap> { self.storage.as_deref() }

    #[must_use]
    pub fn new(catalog: Catalog, directory: Option<PathBuf>) -> Self {
        let storage = catalog.storage_map();
        Self {
            catalog: Some(catalog),
            storage: Some(std::sync::Arc::new(storage)),
            session: ManagerSession::new(directory),
        }
    }

    /// Browse existing folders before an index has been built.
    pub fn live(directory: Option<PathBuf>) -> Self {
        Self { catalog: None, storage: None, session: ManagerSession::new(directory) }
    }

    pub fn set_search_scope(&mut self, scope: LocationScope) {
        self.session.set_search_scope(scope);
    }

    #[must_use]
    pub fn directory(&self) -> Option<&Path> {
        self.session.directory()
    }

    #[must_use]
    pub fn session(&self) -> &ManagerSession {
        &self.session
    }

    #[must_use]
    pub fn location(&self) -> Location {
        self.session.location()
    }

    pub fn select(&mut self, path: Option<PathBuf>) {
        self.session.select(path);
    }

    /// The single dispatch point: every navigation, selection, and scope
    /// change plus every plugin action. `Navigate` accepts live or indexed
    /// directories; plugin actions go to `PluginHost`; everything else falls
    /// through to [`ManagerSession::dispatch`]. Successful moves notify
    /// plugins so Places/recent-locations stay in sync without polling.
    pub fn dispatch(&mut self, plugins: &mut PluginHost, action: Action) -> Result<Outcome> {
        if let Action::Navigate(path) = &action
            && !path.is_dir() && self.catalog.as_ref().and_then(|catalog| catalog.folder(path)).is_none()
        {
            return Err(Error::DirectoryNotIndexed(path.clone()));
        }
        if let Action::Plugin { name, arg } = &action {
            return Ok(Outcome::PluginHandled(plugins.dispatch_action(name, arg)));
        }
        let outcome = self.session.dispatch(action);
        if matches!(
            outcome,
            Outcome::Navigated(_) | Outcome::Moved(_) | Outcome::Located
        ) {
            plugins.notify_navigate(self.directory());
        }
        Ok(outcome)
    }

    pub fn navigate(&mut self, path: PathBuf) -> Result<bool> {
        if !path.is_dir() && self.catalog.as_ref().and_then(|catalog| catalog.folder(&path)).is_none() {
            return Err(Error::DirectoryNotIndexed(path));
        }
        Ok(self.session.navigate(path))
    }

    pub fn back(&mut self) -> Option<PathBuf> {
        self.session.back()
    }

    pub fn forward(&mut self) -> Option<PathBuf> {
        self.session.forward()
    }

    /// Query globally, recursively below the current directory, or only its children.
    pub fn view(&self, query: &str, recursive: bool, mut opts: SearchOpts) -> Result<ManagerView> {
        opts.highlight = false;
        let scoped_directory = (self.session.search_scope() == LocationScope::Directory)
            .then(|| self.directory()).flatten();
        let mut rows: Vec<ManagerRow> = if !recursive && scoped_directory.is_some() {
            crate::live_children(scoped_directory.unwrap(), query, opts, true, true)
                ?.into_iter().map(|row| ManagerRow {
                    id: None, name: row.name, is_dir: row.is_dir,
                    bytes: if row.is_dir {
                        { let sizes=crate::FolderSizes::global(); sizes.request(&row.path); sizes.get(&row.path).or_else(|| self.storage.as_ref().and_then(|map| map.find_indexed(&row.path)).map(|entry| entry.bytes)).unwrap_or(0) }
                    } else { row.size }, path: row.path, entries: 1,
                }).collect()
        } else {
            let catalog = self.catalog.as_ref().ok_or_else(|| Error::Query("Global search needs an index. Build the index first.".into()))?;
            if let Some(directory) = scoped_directory {
                let folder = catalog.folder(directory)
                    .ok_or_else(|| Error::DirectoryNotIndexed(directory.to_path_buf()))?;
                manager_rows(&folder.search_with(query, opts)?)
            } else {
                manager_rows(&catalog.search_with(query, opts)?)
            }
        };
        // ponytail: cached folder weights reorder loaded rows; apply weights before limiting for whole-directory ranking.
        if !recursive && scoped_directory.is_some() && matches!(opts.sort, crate::Sort::Largest | crate::Sort::Smallest) {
            rows.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| {
                if opts.sort == crate::Sort::Largest { b.bytes.cmp(&a.bytes) } else { a.bytes.cmp(&b.bytes) }
            }));
        }
        let folders = rows.iter().filter(|row| row.is_dir).count();
        let files = rows.len() - folders;
        Ok(ManagerView {
            directory: self.directory().map(Path::to_path_buf),
            rows,
            folders,
            files,
        })
    }

    /// Immediate Chart segments for the current directory, or indexed roots globally.
    pub fn chart(&self, global: bool, limit: usize) -> Result<Vec<ManagerRow>> {
        let storage = self.storage.as_ref().ok_or_else(|| Error::Query("Storage analysis needs an index.".into()))?;
        let current = if global {
            None
        } else {
            let directory = self
                .directory()
                .ok_or_else(|| Error::DirectoryNotIndexed(PathBuf::new()))?;
            Some(
                storage
                    .find(directory)
                    .ok_or_else(|| Error::DirectoryNotIndexed(directory.to_path_buf()))?
                    .id,
            )
        };
        Ok(storage
            .children_limited(current, limit)
            .into_iter()
            .map(|entry| ManagerRow {
                id: Some(entry.id),
                name: entry.name,
                path: entry.path,
                is_dir: entry.is_dir,
                bytes: entry.bytes,
                entries: entry.entries,
            })
            .collect())
    }
}

fn manager_rows(hits: &crate::Hits<'_>) -> Vec<ManagerRow> {
    hits.ids()
        .iter()
        .copied()
        .zip(hits.iter())
        .map(|(id, hit)| ManagerRow {
            id: Some(id),
            name: hit.name().to_owned(),
            path: hit.path(),
            is_dir: hit.is_dir(),
            bytes: hit.size(),
            entries: 1,
        })
        .collect()
}
