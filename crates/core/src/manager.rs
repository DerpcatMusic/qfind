use std::path::{Path, PathBuf};

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
}
