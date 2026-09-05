//! TUI workspace: tabs, multi-select, and file operations with undo.
//!
//! The key bindings are published in `docs/tui-keymap.md` and mirrored in
//! the F1 help popup and the footer chips. [`Tab`] owns one tab's query,
//! cursor, and marked set; [`UndoOp`] is the Ctrl+Z stack entry.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One search tab: its own query, cursor, marked (multi-select) set, and the
/// live directory it was browsing, if any.
#[derive(Clone, Debug)]
pub struct Tab {
    pub title: String,
    pub query: String,
    pub selected: usize,
    pub marked: HashSet<PathBuf>,
    pub browser: Option<PathBuf>,
}

impl Tab {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            query: String::new(),
            selected: 0,
            marked: HashSet::new(),
            browser: None,
        }
    }

    /// Toggle `path` in the marked set. Returns true when now marked.
    pub fn toggle_mark(&mut self, path: PathBuf) -> bool {
        if self.marked.remove(&path) {
            false
        } else {
            self.marked.insert(path);
            true
        }
    }

    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    #[must_use]
    pub fn mark_count(&self) -> usize {
        self.marked.len()
    }
}

/// A reversible filesystem mutation for Ctrl+Z.
#[derive(Clone, Debug)]
pub enum UndoOp {
    Trash { staged: PathBuf, orig: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
    Move { from: PathBuf, to: PathBuf },
}

impl UndoOp {
    pub fn describe(&self) -> String {
        match self {
            Self::Trash { orig, .. } => format!("trash {}", orig.display()),
            Self::Rename { from, to } => format!("rename {} -> {}", from.display(), to.display()),
            Self::Created { path } => format!("paste {}", path.display()),
            Self::Move { from, to } => format!("move {} -> {}", from.display(), to.display()),
        }
    }

    pub fn undo(self) -> Result<String, String> {
        match self {
            Self::Trash { staged, orig } => {
                relocate(&staged, &orig)?;
                Ok(format!("restored {}", orig.display()))
            }
            Self::Rename { from, to } => {
                relocate(&to, &from)?;
                Ok(format!("restored {}", from.display()))
            }
            Self::Move { from, to } => {
                relocate(&to, &from)?;
                Ok(format!("moved back {}", from.display()))
            }
            Self::Created { path } => {
                remove_path(&path)?;
                Ok(format!("removed {}", path.display()))
            }
        }
    }
}

/// Move `src` to `dst`, falling back to copy+remove across devices.
pub fn relocate(src: &Path, dst: &Path) -> Result<(), String> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    if src.is_dir() {
        copy_dir_all(src, dst)?;
        remove_path(src)
    } else {
        std::fs::copy(src, dst).map_err(|e| e.to_string())?;
        std::fs::remove_file(src).map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub fn remove_path(path: &Path) -> Result<(), String> {
    if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let to = dst.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfind-workspace-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn marks_toggle_and_clear() {
        let mut tab = Tab::new("t");
        assert!(tab.toggle_mark(PathBuf::from("/a")));
        assert_eq!(tab.mark_count(), 1);
        assert!(!tab.toggle_mark(PathBuf::from("/a")));
        assert_eq!(tab.mark_count(), 0);
        tab.toggle_mark(PathBuf::from("/a"));
        tab.toggle_mark(PathBuf::from("/b"));
        tab.clear_marks();
        assert_eq!(tab.mark_count(), 0);
    }

    #[test]
    fn undo_trash_and_rename_round_trip() {
        let dir = sandbox("trash-rename");
        let file = dir.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        let staged = dir.join("staged.txt");
        relocate(&file, &staged).unwrap();
        UndoOp::Trash {
            staged: staged.clone(),
            orig: file.clone(),
        }
        .undo()
        .unwrap();
        assert!(file.exists() && !staged.exists());

        let renamed = dir.join("g.txt");
        UndoOp::Rename {
            from: file.clone(),
            to: renamed.clone(),
        }
        .undo()
        .unwrap_err();
        relocate(&file, &renamed).unwrap();
        UndoOp::Rename {
            from: file.clone(),
            to: renamed.clone(),
        }
        .undo()
        .unwrap();
        assert!(file.exists() && !renamed.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_created_removes_tree() {
        let dir = sandbox("created");
        let sub = dir.join("sub");
        std::fs::create_dir_all(sub.join("inner")).unwrap();
        UndoOp::Created { path: sub.clone() }.undo().unwrap();
        assert!(!sub.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
