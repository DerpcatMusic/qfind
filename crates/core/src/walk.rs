//! POSIX enumerator: parallel `getdents64` via rustix.
//!
//! Directory type comes from `d_type`; files are statted once so WeightMap and Storage
//! can report real sizes from the persistent Catalog instead of rescanning later.
//! io_uring getdents is not in mainline kernels; this is the fast path that actually exists.

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use rustix::fd::OwnedFd;
#[cfg(unix)]
use rustix::fs::{CWD, Dir, FileType, Mode, OFlags, openat};

use crate::error::Result;
use crate::exclude::Excludes;
use crate::snapshot::Builder;

#[cfg(unix)]
struct Job {
    fd: OwnedFd,
    path: PathBuf,
}

struct Found {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: i64,
}

#[cfg(unix)]
const OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW);

/// Parallel getdents walk. NTFS MFT is a second adapter later; no trait until then.
pub(crate) fn collect(root: &Path, excludes: &Excludes, builder: &mut Builder) -> Result<()> {
    for item in scan(root, excludes) {
        if item.is_dir {
            builder.add_dir(&item.path, root, item.size, item.mtime);
        } else {
            builder.add_file(&item.path, root, item.size, item.mtime);
        }
    }
    Ok(())
}

fn scan(root: &Path, excludes: &Excludes) -> Vec<Found> {
    #[cfg(unix)]
    let found = walk_getdents(root, excludes);
    #[cfg(not(unix))]
    let found = walk_read_dir(root, excludes);
    found
}

#[cfg(unix)]
fn walk_getdents(root: &Path, excludes: &Excludes) -> Vec<Found> {
    let Ok(fd) = openat(CWD, root, OPEN_FLAGS, Mode::empty()) else {
        return Vec::new();
    };
    let nthreads = thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
        .clamp(2, 32);

    let (tx, rx) = crossbeam_channel::unbounded::<Job>();
    let inflight = AtomicUsize::new(1);
    let out = Mutex::new(Vec::<Found>::new());
    // A symlinked folder is a folder, and we follow it — so a link back up the
    // tree would otherwise walk forever. One (dev, ino) per opened directory
    // closes every cycle and stops duplicate work on multiply-linked trees.
    let seen = Mutex::new(Seen::seeded(&fd));
    let _ = tx.send(Job {
        fd,
        path: root.to_path_buf(),
    });

    thread::scope(|scope| {
        for _ in 0..nthreads {
            let rx = rx.clone();
            let tx = tx.clone();
            let inflight = &inflight;
            let out = &out;
            let seen = &seen;
            scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    if inflight.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(8)) {
                        Ok(job) => read_dir(job, excludes, &tx, inflight, seen, &mut local),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
                out.lock().expect("found").extend(local);
            });
        }
        drop(tx);
    });

    out.into_inner().unwrap_or_default()
}

/// Directories already opened, keyed by the inode behind the path we followed.
#[cfg(unix)]
struct Seen(std::collections::HashSet<(u64, u64)>);

#[cfg(unix)]
impl Seen {
    fn seeded(fd: &impl AsFd) -> Self {
        let mut seen = Self(std::collections::HashSet::new());
        seen.admit(fd);
        seen
    }

    fn admit(&mut self, fd: &impl AsFd) -> bool {
        match rustix::fs::fstat(fd) {
            Ok(st) => self.0.insert((st.st_dev, st.st_ino)),
            Err(_) => false,
        }
    }
}

#[cfg(unix)]
fn read_dir(
    job: Job,
    excludes: &Excludes,
    tx: &crossbeam_channel::Sender<Job>,
    inflight: &AtomicUsize,
    seen: &Mutex<Seen>,
    local: &mut Vec<Found>,
) {
    let mut dir = match Dir::read_from(&job.fd) {
        Ok(d) => d,
        Err(_) => {
            inflight.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    };
    loop {
        let entry = match dir.read() {
            None => break,
            Some(Ok(e)) => e,
            Some(Err(_)) => break,
        };
        let raw = entry.file_name().to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        let name = OsStr::from_bytes(raw);
        if excludes.skip_name(name) {
            continue;
        }
        let child = job.path.join(name);
        if excludes.skip(&child) {
            continue;
        }
        // `d_type` is authoritative when it can be; a symlink needs a stat to
        // learn what it points at, and `Unknown` always does.
        let meta = match entry.file_type() {
            FileType::Directory => Some(Meta {
                is_dir: true,
                size: 0,
                mtime: 0,
            }),
            FileType::Unknown | FileType::Symlink => stat_meta(&job.fd, name),
            _ => stat_meta(&job.fd, name),
        };
        let Some(meta) = meta else {
            continue;
        };
        let mut meta = meta;
        // Recurse through symlinked folders too, but only into an inode we have
        // not opened yet. A real directory always has a fresh inode, so this
        // only ever rejects a link that points back at the tree.
        if meta.is_dir
            && let Ok(fd) = openat(&job.fd, name, OPEN_FLAGS, Mode::empty())
            && seen.lock().expect("seen").admit(&fd)
        {
            if let Ok(st) = rustix::fs::fstat(&fd) {
                meta.size = u64::try_from(st.st_size).unwrap_or(0);
                meta.mtime = st.st_mtime;
            }
            inflight.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(Job {
                fd,
                path: child.clone(),
            });
        }
        local.push(Found {
            path: child,
            is_dir: meta.is_dir,
            size: meta.size,
            mtime: meta.mtime,
        });
    }
    inflight.fetch_sub(1, Ordering::SeqCst);
}

#[cfg(unix)]
struct Meta {
    is_dir: bool,
    size: u64,
    mtime: i64,
}

/// `statat` that *follows* a symlink: the user means the target's type, bytes,
/// and age, not the link's own.
#[cfg(unix)]
fn stat_meta(dir_fd: impl AsFd, name: &OsStr) -> Option<Meta> {
    use rustix::fs::AtFlags;
    let st = rustix::fs::statat(dir_fd, name, AtFlags::empty()).ok()?;
    Some(Meta {
        is_dir: FileType::from_raw_mode(st.st_mode) == FileType::Directory,
        size: u64::try_from(st.st_size).unwrap_or(0),
        mtime: st.st_mtime as i64,
    })
}
#[cfg(not(unix))]
fn walk_read_dir(root: &Path, excludes: &Excludes) -> Vec<Found> {
    let nthreads = thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
        .clamp(2, 32);
    let (tx, rx) = crossbeam_channel::unbounded::<PathBuf>();
    let inflight = AtomicUsize::new(1);
    let out = Mutex::new(Vec::<Found>::new());
    let seen = Mutex::new(std::collections::HashSet::new());
    let _ = tx.send(root.to_path_buf());

    thread::scope(|scope| {
        for _ in 0..nthreads {
            let rx = rx.clone();
            let tx = tx.clone();
            let inflight = &inflight;
            let out = &out;
            let seen = &seen;
            scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    if inflight.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(8)) {
                        Ok(path) => {
                            let Ok(entries) = std::fs::read_dir(&path) else {
                                inflight.fetch_sub(1, Ordering::SeqCst);
                                continue;
                            };
                            for entry in entries.flatten() {
                                if excludes.skip_name(&entry.file_name()) {
                                    continue;
                                }
                                let child = entry.path();
                                if excludes.skip(&child) {
                                    continue;
                                }
                                // `metadata` follows symlinks, so a linked
                                // folder is a folder and reports the target's size.
                                let meta = entry.metadata().ok();
                                let is_dir = meta.as_ref().is_some_and(std::fs::Metadata::is_dir);
                                let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
                                let mtime = meta
                                    .as_ref()
                                    .and_then(|m| m.modified().ok())
                                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map_or(0, |age| age.as_secs() as i64);
                                if is_dir && admit(&seen, &child) {
                                    inflight.fetch_add(1, Ordering::SeqCst);
                                    let _ = tx.send(child.clone());
                                }
                                local.push(Found {
                                    path: child,
                                    is_dir,
                                    size,
                                    mtime,
                                });
                            }
                            inflight.fetch_sub(1, Ordering::SeqCst);
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
                out.lock().expect("found").extend(local);
            });
        }
        drop(tx);
    });

    out.into_inner().unwrap_or_default()
}

/// Record a directory as visited so a symlink loop terminates. Without
/// `dev`/`ino` on every platform this degrades to a canonical-path set, which
/// still catches the common `link -> ancestor` case.
#[cfg(not(unix))]
fn admit(seen: &Mutex<std::collections::HashSet<(u64, u64)>>, path: &Path) -> bool {
    let key = path
        .canonicalize()
        .map(|p| (0u64, hash_path(&p)))
        .unwrap_or((0, hash_path(path)));
    seen.lock().expect("seen").insert(key)
}

#[cfg(not(unix))]
fn hash_path(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}
