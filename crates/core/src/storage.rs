use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::catalog::Catalog;
use crate::config::Config;
use crate::snapshot::Entry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageEntry {
    pub id: u32,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub bytes: u64,
    pub entries: u64,
}

/// Compact storage hierarchy over the immutable Catalog. Navigation never touches disk.
pub struct StorageMap {
    catalog: Catalog,
    roots: Vec<u32>,
    children: Vec<Vec<u32>>,
    bytes: Vec<u64>,
    entries: Vec<u64>,
}

impl Catalog {
    #[must_use]
    pub fn storage_map(&self) -> StorageMap {
        StorageMap::new(self.clone())
    }
}

impl StorageMap {
    fn new(catalog: Catalog) -> Self {
        let snapshot = catalog.snapshot();
        let len = snapshot.len() as usize;
        let folders = snapshot.folder_count() as usize;
        let mut roots = Vec::new();
        let mut children = vec![Vec::new(); folders];
        let mut bytes = vec![0u64; len];
        let mut entries = vec![0u64; len];

        for id in 0..snapshot.len() {
            let Some(entry) = snapshot.entry(id) else {
                continue;
            };
            if entry.parent == Entry::ROOT_PARENT {
                roots.push(id);
            } else if let Some(kids) = children.get_mut(entry.parent as usize) {
                kids.push(id);
            }
            if !entry.is_dir() {
                bytes[id as usize] = entry.size;
                entries[id as usize] = 1;
            }
        }

        let config = Config::load();
        let indexed_roots: HashSet<_> = if config.include.is_empty() {
            crate::mounts::discover()
        } else {
            config.include
        }
        .into_iter()
        .collect();
        for id in 0..snapshot.folder_count() {
            if indexed_roots.contains(&snapshot.path(id)) {
                roots.push(id);
            }
        }
        roots.sort_unstable();
        roots.dedup();
        let root_set: HashSet<_> = roots.iter().copied().collect();
        for id in (0..snapshot.len()).rev() {
            if root_set.contains(&id) {
                continue;
            }
            let Some(parent) = snapshot.entry(id).map(|entry| entry.parent) else {
                continue;
            };
            if parent == Entry::ROOT_PARENT {
                continue;
            }
            bytes[parent as usize] = bytes[parent as usize].saturating_add(bytes[id as usize]);
            entries[parent as usize] =
                entries[parent as usize].saturating_add(entries[id as usize]);
        }
        let by_weight = |a: &u32, b: &u32| {
            bytes[*b as usize]
                .cmp(&bytes[*a as usize])
                .then(entries[*b as usize].cmp(&entries[*a as usize]))
                .then(a.cmp(b))
        };
        roots.sort_unstable_by(by_weight);
        for ids in &mut children {
            ids.sort_unstable_by(by_weight);
        }
        Self {
            catalog,
            roots,
            children,
            bytes,
            entries,
        }
    }

    #[must_use]
    pub fn roots(&self) -> Vec<StorageEntry> {
        self.nodes(self.roots.iter().copied())
    }

    #[must_use]
    pub fn children(&self, parent: Option<u32>) -> Vec<StorageEntry> {
        let Some(parent) = parent else {
            return self.roots();
        };
        self.children
            .get(parent as usize)
            .map_or_else(Vec::new, |ids| self.nodes(ids.iter().copied()))
    }

    #[must_use]
    pub fn children_limited(&self, parent: Option<u32>, limit: usize) -> Vec<StorageEntry> {
        let ids = parent
            .and_then(|id| self.children.get(id as usize))
            .unwrap_or(&self.roots);
        self.nodes(ids.iter().copied().take(limit))
    }

    #[must_use]
    pub fn has_children(&self, parent: u32) -> bool {
        self.children
            .get(parent as usize)
            .is_some_and(|ids| !ids.is_empty())
    }

    #[must_use]
    pub fn node(&self, id: u32) -> Option<StorageEntry> {
        let hit = self.catalog.hit(id)?;
        Some(StorageEntry {
            id,
            name: hit.name().to_owned(),
            path: hit.path(),
            is_dir: hit.is_dir(),
            bytes: self.bytes.get(id as usize).copied().unwrap_or(0),
            entries: self.entries.get(id as usize).copied().unwrap_or(0),
        })
    }

    #[must_use]
    pub fn find(&self, path: &Path) -> Option<StorageEntry> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.catalog
            .snapshot()
            .folder_id(&path)
            .and_then(|id| self.node(id))
    }

    #[must_use]
    pub fn parent(&self, id: u32) -> Option<StorageEntry> {
        if self.roots.contains(&id) {
            return None;
        }
        let parent = self.catalog.snapshot().entry(id)?.parent;
        (parent != Entry::ROOT_PARENT)
            .then(|| self.node(parent))
            .flatten()
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.roots
            .iter()
            .filter_map(|&id| self.bytes.get(id as usize))
            .fold(0, |total, bytes| total.saturating_add(*bytes))
    }

    #[must_use]
    pub fn total_entries(&self) -> u64 {
        self.roots
            .iter()
            .filter_map(|&id| self.entries.get(id as usize))
            .fold(0, |total, entries| total.saturating_add(*entries))
    }

    fn nodes(&self, ids: impl Iterator<Item = u32>) -> Vec<StorageEntry> {
        ids.filter_map(|id| self.node(id)).collect()
    }
}
