//! Asynchronous folder-size measurements shared by native surfaces.
//!
//! The catalog remains the fast source of indexed sizes. This worker fills
//! gaps for folders that are outside the catalog without following symlinks
//! or doing filesystem work on a UI thread.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const MAX_PENDING: usize = 128;
const MAX_AGE: u64 = 600;
const MEASURE_LIMIT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct FolderSizes {
    shared: Arc<Shared>,
    sender: SyncSender<Job>,
}

enum Job {
    Measure(PathBuf),
    Persist,
}

struct Shared {
    values: Mutex<BTreeMap<PathBuf, (u64, u64)>>,
    queued: Mutex<HashMap<PathBuf, bool>>,
    failed: Mutex<BTreeMap<PathBuf, u64>>,
    revision: AtomicU64,
}

#[derive(Deserialize)]
struct CacheFile {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    entries: Vec<CacheEntry>,
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    path: String,
    bytes: u64,
    measured: u64,
}

#[derive(Serialize)]
struct CacheOutput {
    version: u8,
    entries: Vec<CacheEntry>,
}

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("qfind/folder-sizes.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl FolderSizes {
    /// Return the process-wide cache and worker shared by every surface.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<FolderSizes> = OnceLock::new();
        GLOBAL.get_or_init(Self::new)
    }

    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = sync_channel(MAX_PENDING);
        let shared = Arc::new(Shared {
            values: Mutex::new(load_cache()),
            queued: Mutex::new(HashMap::new()),
            failed: Mutex::new(BTreeMap::new()),
            revision: AtomicU64::new(1),
        });
        let worker = Arc::clone(&shared);
        let _ = thread::Builder::new()
            .name("qfind-folder-sizes".into())
            .spawn(move || worker_loop(worker, receiver));
        Self { shared, sender }
    }

    #[must_use]
    pub fn get(&self, path: &Path) -> Option<u64> {
        self.shared
            .values
            .lock()
            .ok()?
            .get(path)
            .map(|(bytes, _)| *bytes)
    }

    /// Queue a measurement if the cached value is absent or older than ten minutes.
    /// The bounded channel keeps fast scrolling from creating unbounded work.
    pub fn request(&self, path: &Path) {
        let path = path.to_path_buf();
        let stale = self
            .shared
            .values
            .lock()
            .ok()
            .and_then(|values| values.get(&path).copied())
            .is_none_or(|(_, measured)| now().saturating_sub(measured) > MAX_AGE);
        let failed_recent = self.shared.failed.lock().is_ok_and(|failed| {
            failed
                .get(&path)
                .is_some_and(|at| now().saturating_sub(*at) <= MAX_AGE)
        });
        if !stale || failed_recent {
            return;
        }
        let inserted = self
            .shared
            .queued
            .lock()
            .map(|mut queued| {
                if queued.contains_key(&path) {
                    false
                } else {
                    queued.insert(path.clone(), false);
                    true
                }
            })
            .unwrap_or(false);
        if !inserted {
            return;
        }
        if let Ok(mut failed) = self.shared.failed.lock() {
            failed.remove(&path);
        }
        if let Err(error) = self.sender.try_send(Job::Measure(path.clone())) {
            if let Ok(mut queued) = self.shared.queued.lock() {
                queued.remove(&path);
            }
            if matches!(error, std::sync::mpsc::TrySendError::Disconnected(_)) {
                if let Ok(mut failed) = self.shared.failed.lock() {
                    failed.insert(path, now());
                }
                self.shared.revision.fetch_add(1, Ordering::Release);
            }
        }
    }

    /// Mark a path stale and queue a fresh measurement after a mutation.
    pub fn invalidate(&self, path: &Path) {
        let Ok(mut queued) = self.shared.queued.lock() else {
            return;
        };
        if let Some(dirty) = queued.get_mut(path) {
            *dirty = true;
        }
        if let Ok(mut values) = self.shared.values.lock() {
            if let Some((_, measured)) = values.get_mut(path) {
                *measured = 0;
            }
        }
        if let Ok(mut failed) = self.shared.failed.lock() {
            failed.remove(path);
        }
        self.shared.revision.fetch_add(1, Ordering::Release);
        drop(queued);
        self.request(path);
    }

    /// Seed uncached values from a previous surface's cache.
    ///
    /// Persistence is handed to a worker so importing a legacy cache never
    /// blocks the caller. Existing values always win over imported values.
    pub fn seed(&self, entries: Vec<(PathBuf, u64, u64)>) {
        if entries.is_empty() {
            return;
        }
        let mut changed = false;
        if let Ok(mut values) = self.shared.values.lock() {
            for (path, bytes, measured) in entries {
                if let std::collections::btree_map::Entry::Vacant(slot) = values.entry(path) {
                    slot.insert((bytes, measured));
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
        self.shared.revision.fetch_add(1, Ordering::Release);
        let _ = self.sender.try_send(Job::Persist);
    }

    #[must_use]
    pub fn failed(&self, path: &Path) -> bool {
        self.shared
            .failed
            .lock()
            .is_ok_and(|failed| failed.contains_key(path))
    }

    /// Increments after a successful measurement; cheap for UI redraw polling.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.shared.revision.load(Ordering::Acquire)
    }
}

impl Default for FolderSizes {
    fn default() -> Self {
        Self::new()
    }
}

fn worker_loop(shared: Arc<Shared>, receiver: std::sync::mpsc::Receiver<Job>) {
    while let Ok(job) = receiver.recv() {
        let Job::Measure(path) = job else {
            persist(&shared);
            continue;
        };
        loop {
            let result = measure(&path);
            let Ok(mut queued) = shared.queued.lock() else {
                break;
            };
            if queued.get(&path) == Some(&true) {
                queued.insert(path.clone(), false);
                continue;
            }
            match result {
                Some(bytes) => {
                    if let Ok(mut values) = shared.values.lock() {
                        values.insert(path.clone(), (bytes, now()));
                    }
                    if let Ok(mut failed) = shared.failed.lock() {
                        failed.remove(&path);
                    }
                }
                None => {
                    if let Ok(mut failed) = shared.failed.lock() {
                        failed.insert(path.clone(), now());
                    }
                }
            }
            queued.remove(&path);
            drop(queued);
            shared.revision.fetch_add(1, Ordering::Release);
            persist(&shared);
            break;
        }
    }
}

fn load_cache() -> BTreeMap<PathBuf, (u64, u64)> {
    let Ok(text) = fs::read_to_string(cache_path()) else {
        return BTreeMap::new();
    };
    let Ok(cache) = serde_json::from_str::<CacheFile>(&text) else {
        return BTreeMap::new();
    };
    if cache.version > 1 {
        return BTreeMap::new();
    }
    cache
        .entries
        .into_iter()
        .map(|entry| (PathBuf::from(entry.path), (entry.bytes, entry.measured)))
        .collect()
}

fn persist(shared: &Shared) {
    let entries = shared
        .values
        .lock()
        .map(|values| {
            values
                .iter()
                .map(|(path, (bytes, measured))| CacheEntry {
                    path: path.to_string_lossy().into_owned(),
                    bytes: *bytes,
                    measured: *measured,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let output = match serde_json::to_vec(&CacheOutput {
        version: 1,
        entries,
    }) {
        Ok(output) => output,
        Err(_) => return,
    };
    let path = cache_path();
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(mut temporary) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    use std::io::Write;
    if temporary.write_all(&output).is_err() {
        return;
    }
    let _ = temporary.persist(path);
}

fn measure(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || crate::ops::is_reparse_point(&metadata) {
        return None;
    }
    if !metadata.is_dir() {
        return Some(metadata.len());
    }
    let deadline = Instant::now() + MEASURE_LIMIT;
    let root_device = device(&metadata);
    let mut pending = vec![path.to_path_buf()];
    let mut bytes = 0u64;
    while let Some(directory) = pending.pop() {
        if Instant::now() >= deadline {
            return None;
        }
        let entries = fs::read_dir(directory).ok()?;
        for entry in entries {
            if Instant::now() >= deadline {
                return None;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return None,
            };
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            if metadata.file_type().is_symlink() || crate::ops::is_reparse_point(&metadata) {
                continue;
            }
            if device(&metadata) != root_device {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    Some(bytes)
}

#[cfg(unix)]
fn device(metadata: &fs::Metadata) -> u64 {
    metadata.dev()
}

#[cfg(not(unix))]
fn device(_: &fs::Metadata) -> u64 {
    0
}
