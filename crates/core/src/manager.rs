use std::path::{Path, PathBuf};

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
pub struct Manager {
    catalog: Catalog,
    storage: StorageMap,
    session: ManagerSession,
}

impl Manager {
    #[must_use]
    pub fn new(catalog: Catalog, directory: Option<PathBuf>) -> Self {
        let storage = catalog.storage_map();
        Self {
            catalog,
            storage,
            session: ManagerSession::new(directory),
        }
    }

    #[must_use]
    pub fn directory(&self) -> Option<&Path> {
        self.session.directory()
    }

    pub fn navigate(&mut self, path: PathBuf) -> Result<bool> {
        if self.catalog.folder(&path).is_none() {
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
        let rows = if let Some(directory) = self.directory() {
            let folder = self
                .catalog
                .folder(directory)
                .ok_or_else(|| Error::DirectoryNotIndexed(directory.to_path_buf()))?;
            let hits = if recursive {
                folder.search_with(query, opts)?
            } else {
                folder.search_children_with(query, opts)?
            };
            manager_rows(&hits)
        } else {
            let hits = self.catalog.search_with(query, opts)?;
            manager_rows(&hits)
        };
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
        let current = if global {
            None
        } else {
            let directory = self
                .directory()
                .ok_or_else(|| Error::DirectoryNotIndexed(PathBuf::new()))?;
            Some(
                self.storage
                    .find(directory)
                    .ok_or_else(|| Error::DirectoryNotIndexed(directory.to_path_buf()))?
                    .id,
            )
        };
        Ok(self
            .storage
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
