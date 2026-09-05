//! Shared asynchronous folder measurements for the GTK surface.
//!
//! The worker and account-local cache live in `qfind-core` so native surfaces
//! report the same folder sizes. This adapter keeps the GTK-facing text API
//! and leaves scheduling off the GTK main thread.

use gtk::gio;
use gtk::gio::prelude::FileExt;
use qfind_core::FolderSizes;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Sizes {
    inner: FolderSizes,
}

impl Sizes {
    pub fn new() -> Self {
        let inner = FolderSizes::global().clone();
        import_legacy_cache(&inner);
        Self { inner }
    }

    pub fn get(&self, path: &Path) -> Option<u64> {
        self.inner.get(path)
    }

    pub fn request(&self, path: &Path) {
        self.inner.request(path);
    }

    pub fn failed(&self, path: &Path) -> bool {
        self.inner.failed(path)
    }

    /// Changes only after a measurement succeeds, allowing cheap redraw polling.
    pub fn revision(&self) -> u64 {
        self.inner.revision()
    }

    pub fn text(&self, path: &Path) -> String {
        self.inner.request(path);
        self.get(path)
            .map(crate::actions::human_size)
            .unwrap_or_else(|| {
                if self.failed(path) {
                    "Unavailable"
                } else {
                    "Measuring…"
                }
                .into()
            })
    }
}

fn import_legacy_cache(inner: &FolderSizes) {
    use std::sync::OnceLock;
    static IMPORTED: OnceLock<()> = OnceLock::new();
    IMPORTED.get_or_init(|| {
        let path = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("qfind/folder-sizes.tsv");
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let entries = text
            .lines()
            .filter_map(|line| {
                let [uri, bytes, measured] =
                    line.split('\t').collect::<Vec<_>>().try_into().ok()?;
                let path: PathBuf = gio::File::for_uri(uri).path()?;
                Some((path, bytes.parse().ok()?, measured.parse().ok()?))
            })
            .collect();
        inner.seed(entries);
    });
}

impl Default for Sizes {
    fn default() -> Self {
        Self::new()
    }
}
