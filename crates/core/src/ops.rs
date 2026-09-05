//! Local filesystem mutations for manager frontends.
//!
//! Frontends (GTK/TUI) call these instead of implementing recursive
//! copy, collision policy, or trash handling themselves. Every function
//! returns [`crate::Result`]. The Catalog snapshot is immutable, so callers
//! [`crate::Catalog::rebuild`] (or apply the returned [`Mutation`] to a
//! live-delta overlay) after a successful mutation.
//!
//! Paths are plain local paths: the same [`PathBuf`] values carried by
//! [`crate::Hit::path`] and [`crate::StorageEntry::path`]. No new
//! file-model types are introduced.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Describes one completed mutation for a live-delta overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    Created(PathBuf),
    CreatedDir(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Copied { from: PathBuf, to: PathBuf },
    Moved { from: PathBuf, to: PathBuf },
    Deleted(PathBuf),
    Trashed { from: PathBuf, to: PathBuf },
    Restored { from: PathBuf, to: PathBuf },
}

pub(crate) fn refresh_sizes(path: &Path) {
    let sizes = crate::FolderSizes::global();
    if let Some(parent) = path.parent() { sizes.invalidate(parent); }
    if path.is_dir() { sizes.invalidate(path); }
}

pub(crate) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn invalid_input(path: &Path, what: &str) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, what),
    }
}

fn check_source(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(invalid_input(path, "empty path"));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Error::NotFound(path.to_path_buf())),
        Err(e) => Err(Error::io(path, e)),
    }
}

fn check_dest_free(dest: &Path) -> Result<()> {
    if dest.as_os_str().is_empty() {
        return Err(invalid_input(dest, "empty path"));
    }
    match fs::symlink_metadata(dest) {
        Ok(_) => Err(Error::AlreadyExists(dest.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(dest, e)),
    }
}

fn check_dest_parent(dest: &Path) -> Result<()> {
    match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => match fs::metadata(parent) {
            Ok(md) if md.is_dir() => Ok(()),
            Ok(_) => Err(invalid_input(dest, "parent is not a directory")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(Error::NotFound(parent.to_path_buf()))
            }
            Err(e) => Err(Error::io(parent, e)),
        },
        _ => Ok(()),
    }
}

/// Create an empty file. Parent directories are created as needed.
/// Fails with [`Error::AlreadyExists`] when the path exists.
pub fn create_file(path: impl AsRef<Path>) -> Result<Mutation> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(invalid_input(path, "empty path"));
    }
    check_dest_free(path)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    fs::File::create_new(path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::AlreadyExists(path.to_path_buf())
        } else {
            Error::io(path, e)
        }
    })?;
    refresh_sizes(path);
    Ok(Mutation::Created(path.to_path_buf()))
}

/// Create a directory and any missing parents.
/// Fails with [`Error::AlreadyExists`] when the path exists.
pub fn create_dir(path: impl AsRef<Path>) -> Result<Mutation> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(invalid_input(path, "empty path"));
    }
    check_dest_free(path)?;
    fs::create_dir_all(path).map_err(|e| Error::io(path, e))?;
    refresh_sizes(path);
    Ok(Mutation::CreatedDir(path.to_path_buf()))
}

/// Rename a file or directory. Fails when the source is missing,
/// the destination exists, or the destination parent is missing.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<Mutation> {
    let (from, to) = (from.as_ref(), to.as_ref());
    check_source(from)?;
    check_dest_free(to)?;
    check_dest_parent(to)?;
    fs::rename(from, to).map_err(|e| Error::io(to, e))?;
    refresh_sizes(from);
    refresh_sizes(to);
    Ok(Mutation::Renamed {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })
}

fn copy_file_one(from: &Path, to: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(from).map_err(|e| Error::io(from, e))?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(from).map_err(|e| Error::io(from, e))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, to).map_err(|e| Error::io(to, e))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
            if metadata.file_type().is_symlink_dir() { symlink_dir(target, to) }
            else { symlink_file(target, to) }.map_err(|e| Error::io(to, e))?;
        }
        #[cfg(not(any(unix, windows)))]
        return Err(invalid_input(to, "symlink copying is unsupported on this platform"));
    } else {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
            let destination: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: Both paths are terminated UTF-16. Fail-if-exists preserves collision policy.
            // CopyFileW also preserves Windows streams and file attributes.
            let copied=unsafe { windows_sys::Win32::Storage::FileSystem::CopyFileW(from.as_ptr(),destination.as_ptr(),1) };
            if copied==0 {return Err(Error::io(to,io::Error::last_os_error()));}
        }
        #[cfg(not(windows))]
        {
            let mut source = fs::File::open(from).map_err(|e| Error::io(from, e))?;
            let mut destination = fs::File::create_new(to).map_err(|e| Error::io(to, e))?;
            io::copy(&mut source, &mut destination).map_err(|e| Error::io(to, e))?;
            fs::set_permissions(to, metadata.permissions()).map_err(|e| Error::io(to, e))?;
        }
    }
    Ok(())
}

fn copy_dir_all(from: &Path, to: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(from).map_err(|e| Error::io(from, e))?;
    if is_reparse_point(&metadata) && !metadata.file_type().is_symlink() {
        return Err(invalid_input(from, "cannot recursively copy a Windows reparse point"));
    }
    let source = fs::canonicalize(from).map_err(|e| Error::io(from, e))?;
    let parent = to.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|e| Error::io(parent, e))?;
    if parent.starts_with(&source) {
        return Err(invalid_input(to, "cannot copy a directory into itself"));
    }
    fs::create_dir(to).map_err(|e| Error::io(to, e))?;
    let entries = fs::read_dir(from).map_err(|e| Error::io(from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let src = entry.path();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| Error::io(&src, e))?;
        if kind.is_dir() {
            copy_dir_all(&src, &dest)?;
        } else {
            copy_file_one(&src, &dest)?;
        }
    }
    Ok(())
}

/// Copy a file or directory tree to a free destination.
/// Fails with [`Error::AlreadyExists`] when the destination exists.
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<Mutation> {
    let (from, to) = (from.as_ref(), to.as_ref());
    check_source(from)?;
    check_dest_free(to)?;
    check_dest_parent(to)?;
    let meta = fs::symlink_metadata(from).map_err(|e| Error::io(from, e))?;
    let result = if meta.is_dir() {
        copy_dir_all(from, to)
    } else {
        copy_file_one(from, to)
    };
    refresh_sizes(to);
    result?;
    Ok(Mutation::Copied {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })
}

/// Rename, falling back to copy+delete when `to` is on another Mount.
fn move_tree(from: &Path, to: &Path) -> Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            let meta = fs::symlink_metadata(from).map_err(|e2| Error::io(from, e2))?;
            if meta.is_dir() {
                copy_dir_all(from, to)?;
                fs::remove_dir_all(from).map_err(|e2| Error::io(from, e2))?;
            } else {
                copy_file_one(from, to)?;
                fs::remove_file(from).map_err(|e2| Error::io(from, e2))?;
            }
            Ok(())
        }
        Err(e) => Err(Error::io(to, e)),
    }
}

/// Move a file or directory tree. Falls back to copy+delete across Mounts.
pub fn move_path(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<Mutation> {
    let (from, to) = (from.as_ref(), to.as_ref());
    check_source(from)?;
    check_dest_free(to)?;
    check_dest_parent(to)?;
    let result = move_tree(from, to);
    refresh_sizes(from);
    refresh_sizes(to);
    result?;
    Ok(Mutation::Moved {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })
}

/// Delete a file, symlink, or directory tree. Missing paths report
/// [`Error::NotFound`].
pub fn delete(path: impl AsRef<Path>) -> Result<Mutation> {
    let path = path.as_ref();
    check_source(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|e| Error::io(path, e))?;
    } else {
        fs::remove_file(path).map_err(|e| Error::io(path, e))?;
    }
    refresh_sizes(path);
    Ok(Mutation::Deleted(path.to_path_buf()))
}

/// Trash root: `$XDG_DATA_HOME/qfind/Trash/files` (freedesktop-style).
#[must_use]
pub fn trash_root() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("qfind").join("Trash").join("files")
}

fn unique_in(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let mut candidate = dir.join(name);
    let mut n: u32 = 1;
    while candidate.exists() {
        let suffixed = format!("{}.{n}", name.to_string_lossy());
        candidate = dir.join(suffixed);
        n = n.saturating_add(1);
        if n == u32::MAX {
            break;
        }
    }
    candidate
}

/// Move a path into the trash root, returning the trashed location.
/// Colliding names get a numeric suffix.
pub fn trash(path: impl AsRef<Path>) -> Result<(PathBuf, Mutation)> {
    trash_into(&trash_root(), path)
}

/// [`trash`] with an explicit root. Tests use a tempdir so they never touch
/// the real trash.
pub fn trash_into(root: &Path, path: impl AsRef<Path>) -> Result<(PathBuf, Mutation)> {
    let path = path.as_ref();
    check_source(path)?;
    fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
    let name = match path.file_name() {
        Some(n) => n.to_os_string(),
        None => return Err(invalid_input(path, "path has no file name")),
    };
    let dest = unique_in(root, &name);
    move_tree(path, &dest)?;
    refresh_sizes(path);
    refresh_sizes(&dest);
    Ok((
        dest.clone(),
        Mutation::Trashed {
            from: path.to_path_buf(),
            to: dest,
        },
    ))
}

/// Restore a trashed path to `original`. Fails with
/// [`Error::AlreadyExists`] when something now occupies `original`.
pub fn restore(trashed: impl AsRef<Path>, original: impl AsRef<Path>) -> Result<Mutation> {
    let (trashed, original) = (trashed.as_ref(), original.as_ref());
    check_source(trashed)?;
    check_dest_free(original)?;
    check_dest_parent(original)?;
    move_tree(trashed, original)?;
    refresh_sizes(trashed);
    refresh_sizes(original);
    Ok(Mutation::Restored {
        from: trashed.to_path_buf(),
        to: original.to_path_buf(),
    })
}

/// Delete the file or folder described by a [`crate::StorageEntry`].
pub fn delete_entry(entry: &crate::StorageEntry) -> Result<Mutation> {
    delete(&entry.path)
}

/// Trash the file or folder described by a [`crate::StorageEntry`].
pub fn trash_entry(entry: &crate::StorageEntry) -> Result<(PathBuf, Mutation)> {
    trash(&entry.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn create_rename_delete_file() {
        let t = tmp();
        let f = t.path().join("sub").join("a.txt");
        let m = create_file(&f).unwrap();
        assert_eq!(m, Mutation::Created(f.clone()));
        assert!(f.exists());
        assert!(matches!(create_file(&f), Err(Error::AlreadyExists(_))));

        let g = t.path().join("b.txt");
        rename(&f, &g).unwrap();
        assert!(!f.exists() && g.exists());
        assert!(matches!(rename(&f, &g), Err(Error::NotFound(_))));
        assert!(matches!(rename(&g, &g), Err(Error::AlreadyExists(_))));

        delete(&g).unwrap();
        assert!(!g.exists());
        assert!(matches!(delete(&g), Err(Error::NotFound(_))));
    }

    #[test]
    fn mkdir_copy_move_tree() {
        let t = tmp();
        let d = t.path().join("d");
        create_dir(&d).unwrap();
        assert!(d.is_dir());
        assert!(matches!(create_dir(&d), Err(Error::AlreadyExists(_))));

        let src = d.join("src");
        create_dir(&src).unwrap();
        create_file(src.join("f.txt")).unwrap();
        fs::write(src.join("f.txt"), b"hi").unwrap();

        let cp = t.path().join("cp");
        copy(&src, &cp).unwrap();
        assert_eq!(fs::read(cp.join("f.txt")).unwrap(), b"hi");

        let mv = t.path().join("mv");
        move_path(&cp, &mv).unwrap();
        assert!(mv.join("f.txt").exists() && !cp.exists());
    }

    #[test]
    fn trash_and_restore_round_trip() {
        let t = tmp();
        let can = t.path().join("trash");
        let f = t.path().join("gone.txt");
        create_file(&f).unwrap();
        fs::write(&f, b"x").unwrap();
        let (trashed, m) = trash_into(&can, &f).unwrap();
        assert!(!f.exists() && trashed.exists());
        assert_eq!(
            m,
            Mutation::Trashed {
                from: f.clone(),
                to: trashed.clone()
            }
        );
        restore(&trashed, &f).unwrap();
        assert!(f.exists() && !trashed.exists());
    }

    #[test]
    fn trash_uniquifies_colliding_names() {
        let t = tmp();
        let can = t.path().join("trash");
        let a = t.path().join("a");
        let b = t.path().join("b");
        create_dir(&a).unwrap();
        create_dir(&b).unwrap();
        create_file(a.join("same.txt")).unwrap();
        create_file(b.join("same.txt")).unwrap();
        let (t1, _) = trash_into(&can, a.join("same.txt")).unwrap();
        let (t2, _) = trash_into(&can, b.join("same.txt")).unwrap();
        assert_ne!(t1, t2);
        assert!(t1.exists() && t2.exists());
    }

    #[test]
    fn copy_rejects_occupied_dest_and_move_reports_missing() {
        let t = tmp();
        let f = t.path().join("f.txt");
        create_file(&f).unwrap();
        assert!(matches!(copy(&f, &f), Err(Error::AlreadyExists(_))));
        assert!(matches!(
            move_path(t.path().join("nope"), &f),
            Err(Error::NotFound(_))
        ));
    }
}
