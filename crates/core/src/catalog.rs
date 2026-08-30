use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Result;
use crate::exclude::Excludes;
use crate::mounts;
use crate::search;
use crate::snapshot::{Builder, Snapshot};
use crate::walk;

/// How to Rebuild the Catalog from disk.
#[derive(Clone, Debug)]
pub struct Rebuild {
    snapshot: PathBuf,
    roots: Option<Vec<PathBuf>>,
    extra_excludes: Vec<String>,
    extra_exclude_paths: Vec<PathBuf>,
}

impl Rebuild {
    #[must_use]
    pub fn new(snapshot: impl Into<PathBuf>) -> Self {
        Self {
            snapshot: snapshot.into(),
            roots: None,
            extra_excludes: Vec::new(),
            extra_exclude_paths: Vec::new(),
        }
    }

    /// Limit Rebuild to these Mounts. `None` (default) discovers local Mounts.
    #[must_use]
    pub fn roots(mut self, roots: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        self.roots = Some(roots.into_iter().map(Into::into).collect());
        self
    }

    #[must_use]
    pub fn exclude(mut self, pattern: impl Into<String>) -> Self {
        self.extra_excludes.push(pattern.into());
        self
    }

    #[must_use]
    pub fn exclude_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.extra_exclude_paths.push(path.into());
        self
    }
}

/// The Catalog: Rebuild from Mounts, Query by filename.
#[derive(Clone)]
pub struct Catalog {
    path: PathBuf,
    snapshot: Arc<Snapshot>,
}

impl Catalog {
    /// Open an existing snapshot.
    ///
    /// # Errors
    /// Returns [`Error::Snapshot`] or [`Error::Io`] if the file is missing or corrupt.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let snapshot = Snapshot::open_mmap(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            snapshot: Arc::new(snapshot),
        })
    }

    /// Rebuild from the live filesystem and open the result.
    ///
    /// # Errors
    /// Returns I/O or Exclude errors. Permission errors on individual files are skipped.
    pub fn rebuild(rebuild: Rebuild) -> Result<Self> {
        let excludes = Excludes::with_paths(&rebuild.extra_excludes, &rebuild.extra_exclude_paths)?;
        let roots = match rebuild.roots {
            Some(r) => r,
            None => mounts::discover()
                .into_iter()
                .filter(|p| !excludes.skip(p) && !mounts::is_under_skip_mount(p))
                .collect(),
        };
        let mut builder = Builder::new();
        for root in &roots {
            if !root.exists() {
                continue;
            }
            walk::collect(root, &excludes, &mut builder)?;
        }
        builder.write(&rebuild.snapshot)?;
        Self::open(&rebuild.snapshot)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn len(&self) -> u32 {
        self.snapshot.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn folder_count(&self) -> u32 {
        self.snapshot.folder_count()
    }

    #[must_use]
    pub fn file_count(&self) -> u32 {
        self.snapshot.file_count()
    }

    /// Filter the Catalog with a Query string (highlight on, no limit).
    ///
    /// # Errors
    /// Returns [`Error::Query`] for a malformed glob.
    pub fn search(&self, query: &str) -> Result<Hits<'_>> {
        self.search_with(
            query,
            crate::SearchOpts {
                highlight: true,
                ..crate::SearchOpts::default()
            },
        )
    }

    /// Filter with scope, class, sort, and limit.
    ///
    /// # Errors
    /// Returns [`Error::Query`] for a malformed glob.
    pub fn search_with(&self, query: &str, opts: crate::SearchOpts) -> Result<Hits<'_>> {
        let ranked = search::search(&self.snapshot, query, opts)?;
        Ok(Hits {
            catalog: self,
            ids: ranked.ids,
            indices: ranked.indices,
        })
    }

    /// Filter while allowing a caller to stop stale Query work.
    ///
    /// # Errors
    /// Returns [`Error::Cancelled`](crate::Error::Cancelled) when `cancelled` becomes true.
    pub fn search_with_cancel(
        &self,
        query: &str,
        opts: crate::SearchOpts,
        cancelled: impl Fn() -> bool + Sync,
    ) -> Result<Hits<'_>> {
        let ranked = search::search_with_cancel(&self.snapshot, query, opts, true, &cancelled)?;
        Ok(Hits {
            catalog: self,
            ids: ranked.ids,
            indices: ranked.indices,
        })
    }

    /// Filter while optionally hiding dotfiles and allowing stale work to stop.
    ///
    /// # Errors
    /// Returns [`Error::Cancelled`](crate::Error::Cancelled) when `cancelled` becomes true.
    pub fn search_with_hidden_cancel(
        &self,
        query: &str,
        opts: crate::SearchOpts,
        show_hidden: bool,
        cancelled: impl Fn() -> bool + Sync,
    ) -> Result<Hits<'_>> {
        let ranked =
            search::search_with_cancel(&self.snapshot, query, opts, show_hidden, &cancelled)?;
        Ok(Hits {
            catalog: self,
            ids: ranked.ids,
            indices: ranked.indices,
        })
    }

    pub fn hit(&self, id: u32) -> Option<Hit<'_>> {
        self.snapshot.entry(id).map(|_| Hit {
            catalog: self,
            id,
            indices: Vec::new(),
        })
    }

    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Touch the packed letter-mask so the first Query does not pay for it.
    pub fn warm(&self) {
        let _ = self.snapshot.letter_mask();
    }
}

/// Default snapshot path: `$XDG_CACHE_HOME/qfind/catalog`.
#[must_use]
pub fn default_snapshot_path() -> PathBuf {
    let cache = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    cache.join("qfind").join("catalog")
}

/// Hits from one Query.
pub struct Hits<'a> {
    catalog: &'a Catalog,
    ids: Vec<u32>,
    indices: Vec<Vec<u32>>,
}

impl<'a> Hits<'a> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    #[must_use]
    pub fn get(&self, i: usize) -> Option<Hit<'a>> {
        let id = *self.ids.get(i)?;
        Some(Hit {
            catalog: self.catalog,
            id,
            indices: self.indices.get(i).cloned().unwrap_or_default(),
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = Hit<'a>> + '_ {
        self.ids.iter().enumerate().map(|(i, id)| Hit {
            catalog: self.catalog,
            id: *id,
            indices: self.indices.get(i).cloned().unwrap_or_default(),
        })
    }

    /// Positions in this Hit list, for a caller that virtualizes the view.
    #[must_use]
    pub fn at(&self, index: usize) -> Option<Hit<'a>> {
        self.get(index)
    }

    #[must_use]
    pub fn ids(&self) -> &[u32] {
        &self.ids
    }
}

/// One file or folder from the Catalog.
#[derive(Clone)]
pub struct Hit<'a> {
    catalog: &'a Catalog,
    id: u32,
    indices: Vec<u32>,
}

impl Hit<'_> {
    #[must_use]
    pub fn name(&self) -> &str {
        self.catalog
            .snapshot()
            .entry(self.id)
            .map(|e| self.catalog.snapshot().name(e))
            .unwrap_or("")
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.catalog.snapshot().path(self.id)
    }

    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.catalog
            .snapshot()
            .entry(self.id)
            .is_some_and(|e| e.is_dir())
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.catalog
            .snapshot()
            .entry(self.id)
            .map(|e| e.size)
            .unwrap_or(0)
    }

    #[must_use]
    pub fn mtime(&self) -> i64 {
        self.catalog
            .snapshot()
            .entry(self.id)
            .map(|e| e.mtime)
            .unwrap_or(0)
    }

    /// Character indices in [`Self::name`] that matched the Query (fuzzy highlight).
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
}
