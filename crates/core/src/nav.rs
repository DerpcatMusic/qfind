//! Shared navigation model: Location, breadcrumbs, tree expansion.
//!
//! Every frontend (GTK, TUI, CLI `--here`, native FFI) renders this state.
//! None of them reimplements history, breadcrumb splitting, or tree folding.
//! History itself stays in [`crate::ManagerSession`]; this module is the
//! value types both sides speak.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::view::{Flat, Stem, walk_visible};

/// Where the manager is standing: the whole Catalog, or inside one directory.
///
/// Whether a directory means "recursive subtree" (Qfind mode) or "direct
/// children" (Classic browse) is [`crate::BrowseMode`], not this type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Location {
    /// Global Catalog Queries.
    #[default]
    Global,
    /// Inside `path`.
    Directory(PathBuf),
}

impl Location {
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Global => None,
            Self::Directory(path) => Some(path),
        }
    }

    #[must_use]
    pub fn is_global(&self) -> bool {
        matches!(self, Self::Global)
    }
}

/// One clickable breadcrumb segment: display name plus the path it jumps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub name: String,
    pub path: PathBuf,
}

/// Split `path` into root-to-leaf crumbs: `/a/b` -> `/`, `/a`, `/a/b`.
/// `.` components are skipped, `..` pops. Empty input yields no crumbs.
#[must_use]
pub fn breadcrumb(path: &Path) -> Vec<Crumb> {
    let mut crumbs = Vec::new();
    let mut acc = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => acc.push(prefix.as_os_str()),
            Component::RootDir => {
                acc.push("/");
                crumbs.push(Crumb {
                    name: "/".into(),
                    path: acc.clone(),
                });
            }
            Component::Normal(part) => {
                acc.push(part);
                crumbs.push(Crumb {
                    name: part.to_string_lossy().into_owned(),
                    path: acc.clone(),
                });
            }
            Component::CurDir => {}
            Component::ParentDir => {
                acc.pop();
            }
        }
    }
    crumbs
}

/// Shared expand/collapse state over folded [`Stem`] trees.
///
/// GTK kept its own `collapsed: HashSet<String>` and TUI kept ad-hoc
/// folder/item panes; both render `TreeState::visible` now so the tree
/// agrees across frontends. Everything starts expanded.
#[derive(Clone, Debug, Default)]
pub struct TreeState {
    collapsed: HashSet<String>,
}

impl TreeState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn expand(&mut self, path: &str) {
        self.collapsed.remove(path);
    }

    pub fn collapse(&mut self, path: &str) {
        self.collapsed.insert(path.to_string());
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
    }

    #[must_use]
    pub fn is_expanded(&self, path: &str) -> bool {
        !self.collapsed.contains(path)
    }

    /// Rows the adapter should draw, depth-first.
    #[must_use]
    pub fn visible(&self, stems: &[Stem]) -> Vec<Flat> {
        walk_visible(stems, &|p| self.is_expanded(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{HitRef, fold_stems};

    #[test]
    fn crumbs_cover_root_to_leaf() {
        let crumbs = breadcrumb(Path::new("/a/b"));
        let names: Vec<&str> = crumbs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["/", "a", "b"]);
        assert_eq!(crumbs[2].path, PathBuf::from("/a/b"));
        assert!(breadcrumb(Path::new("")).is_empty());
    }

    #[test]
    fn tree_collapse_hides_children() {
        let items = [
            HitRef {
                id: Some(1),
                path: "/a/b/c.txt".into(),
                is_dir: false,
                weight: 1,
            },
            HitRef {
                id: Some(2),
                path: "/a/d.txt".into(),
                is_dir: false,
                weight: 1,
            },
        ];
        let stems = fold_stems(&items);
        let mut state = TreeState::new();
        assert!(
            state.visible(&stems).iter().any(|f| f.stem.name == "c.txt"),
            "fresh tree shows files, not only folders"
        );
        state.collapse("/a");
        assert!(!state.is_expanded("/a"));
        let shown = state.visible(&stems);
        assert!(!shown.iter().any(|f| f.stem.name == "c.txt"));
        assert!(shown.iter().any(|f| f.stem.name == "a"));
        state.toggle("/a");
        assert!(state.visible(&stems).iter().any(|f| f.stem.name == "c.txt"));
    }
}
