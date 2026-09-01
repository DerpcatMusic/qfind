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
}

#[cfg(unix)]
const OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW);

/// Parallel getdents walk. NTFS MFT is a second adapter later; no trait until then.
pub(crate) fn collect(root: &Path, excludes: &Excludes, builder: &mut Builder) -> Result<()> {
    let found = scan(root, excludes);
    for item in found {
        if item.is_dir {
            builder.add_dir(&item.path, root, 0, 0);
        } else {
            builder.add_file(&item.path, root, item.size, 0);
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
            scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    if inflight.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(8)) {
                        Ok(job) => {
                            read_dir(job, excludes, &tx, inflight, &mut local);
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

#[cfg(unix)]
fn read_dir(
    job: Job,
    excludes: &Excludes,
    tx: &crossbeam_channel::Sender<Job>,
    inflight: &AtomicUsize,
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
        let (is_dir, size) = match entry.file_type() {
            FileType::Directory => (true, 0),
            FileType::Unknown => stat_info(&job.fd, name),
            _ => (false, file_size(&job.fd, name)),
        };
        if is_dir {
            if let Ok(fd) = openat(&job.fd, name, OPEN_FLAGS, Mode::empty()) {
                inflight.fetch_add(1, Ordering::SeqCst);
                let _ = tx.send(Job {
                    fd,
                    path: child.clone(),
                });
            }
        }
        local.push(Found {
            path: child,
            is_dir,
            size,
        });
    }
    inflight.fetch_sub(1, Ordering::SeqCst);
}

#[cfg(unix)]
fn stat_info(dir_fd: impl AsFd, name: &OsStr) -> (bool, u64) {
    use rustix::fs::{AtFlags, statat};
    match statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(st) => (
            FileType::from_raw_mode(st.st_mode) == FileType::Directory,
            u64::try_from(st.st_size).unwrap_or(0),
        ),
        Err(_) => (false, 0),
    }
}

#[cfg(unix)]
fn file_size(dir_fd: impl AsFd, name: &OsStr) -> u64 {
    stat_info(dir_fd, name).1
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
    let _ = tx.send(root.to_path_buf());

    thread::scope(|scope| {
        for _ in 0..nthreads {
            let rx = rx.clone();
            let tx = tx.clone();
            let inflight = &inflight;
            let out = &out;
            scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    if inflight.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(8)) {
                        Ok(path) => {
                            let Ok(entries) = std::fs::read_dir(path) else {
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
                                let metadata = entry.metadata().ok();
                                let is_dir = metadata.as_ref().is_some_and(|kind| kind.is_dir());
                                let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
                                if is_dir {
                                    inflight.fetch_add(1, Ordering::SeqCst);
                                    let _ = tx.send(child.clone());
                                }
                                local.push(Found {
                                    path: child,
                                    is_dir,
                                    size,
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
