use std::collections::hash_map::DefaultHasher;
#[cfg(unix)]
use std::ffi::OsString;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use compress_tools::{ArchiveContents, ArchiveIterator};
#[cfg(unix)]
use simple_archive::writer::ArchiveWriter;

const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const STAGING_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub contents: PathBuf,
    pub source: PathBuf,
}

pub fn is_archive(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| crate::FileClass::Archive.matches(name, false))
}

pub fn unpack(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize().context("archive does not exist")?;
    let metadata = path.metadata().context("cannot read archive metadata")?;
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    metadata.len().hash(&mut hash);
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_nanos())
        .hash(&mut hash);

    let cache = dirs::cache_dir()
        .context("no user cache directory")?
        .join("qfind/archives");
    fs::create_dir_all(&cache)?;
    let key = format!("{:016x}", hash.finish());
    let destination = cache.join(&key);
    let contents = destination.join("contents");
    let ready = destination.join("ready");
    if ready.is_file() && contents.is_dir() {
        fs::write(&ready, [])?;
        return Ok(contents);
    }

    let staging = cache.join(format!(
        "{key}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&staging)?;
    let staging_contents = staging.join("contents");
    fs::create_dir(&staging_contents)?;
    if let Err(error) = unpack_into(&path, &staging_contents, metadata.len()) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    write_source(&staging.join("source"), &path)?;
    fs::write(staging.join("ready"), [])?;
    match fs::rename(&staging, &destination) {
        Ok(()) => {}
        Err(_) if destination.is_dir() => {
            let _ = fs::remove_dir_all(&staging);
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error.into());
        }
    }
    crate::ops::refresh_sizes(&contents);
    Ok(contents)
}

pub fn workspace(path: &Path) -> Option<Workspace> {
    let cache = dirs::cache_dir()
        .map(|path| path.join("qfind/archives"))?
        .canonicalize()
        .ok()?;
    let path = path.canonicalize().ok()?;
    let contents = path
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "contents"))?
        .to_path_buf();
    let root = contents.parent()?.to_path_buf();
    if root.parent()? != cache || !root.join("ready").is_file() {
        return None;
    }
    let source = read_source(&root.join("source")).ok()?;
    Some(Workspace {
        root,
        contents,
        source,
    })
}

pub fn can_repack(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.ends_with(".zip")
        || name.ends_with(".7z")
        || name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tbz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".txz")
        || name.ends_with(".tar.zst")
        || name.ends_with(".tzst")
}

pub fn repack(workspace: &Workspace) -> Result<()> {
    if !can_repack(&workspace.source) {
        bail!("this archive format is read-only")
    }
    let parent = workspace
        .source
        .parent()
        .context("archive has no parent directory")?;
    let name = workspace
        .source
        .file_name()
        .and_then(|name| name.to_str())
        .context("archive name is not valid UTF-8")?;
    let temporary = parent.join(format!(
        ".qfind-repack-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let permissions = workspace.source.metadata()?.permissions();
    let result = repack_into(workspace, &temporary);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::set_permissions(&temporary, permissions)?;
    File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, &workspace.source)?;
    crate::ops::refresh_sizes(&workspace.source);
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    fs::write(workspace.root.join("ready"), [])?;
    Ok(())
}

pub fn prune_cache(protected: Option<&Path>) {
    let Some(cache) = dirs::cache_dir().map(|path| path.join("qfind/archives")) else {
        return;
    };
    prune_cache_at(&cache, protected, SystemTime::now());
}

fn prune_cache_at(cache: &Path, protected: Option<&Path>, now: SystemTime) {
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if protected.is_some_and(|protected| protected.starts_with(&path)) {
            continue;
        }
        let max_age = if entry.file_name().to_string_lossy().contains(".tmp-") {
            STAGING_MAX_AGE
        } else {
            CACHE_MAX_AGE
        };
        let modified = path
            .join("ready")
            .metadata()
            .or_else(|_| entry.metadata())
            .and_then(|meta| meta.modified());
        if modified
            .ok()
            .and_then(|time| now.duration_since(time).ok())
            .is_some_and(|age| age > max_age)
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(unix)]
fn repack_into(workspace: &Workspace, destination: &Path) -> Result<()> {
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut writer = ArchiveWriter::new(file)?;
    configure_writer(&mut writer, &workspace.source)?;
    writer.open()?;
    add_tree(&mut writer, &workspace.contents, &workspace.contents)?;
    drop(writer);
    compress_tools::list_archive_entries(File::open(destination)?)?;
    Ok(())
}

/// Create a new archive without replacing an existing destination.
pub fn compress(paths: &[PathBuf], destination: &Path) -> Result<()> {
    let root = paths
        .first()
        .and_then(|path| path.parent())
        .context("No files selected")?;
    let resolved_destination = destination
        .parent()
        .context("Destination needs a parent folder")?
        .canonicalize()?
        .join(
            destination
                .file_name()
                .context("Destination needs a filename")?,
        );
    for path in paths {
        if path.parent() != Some(root) || resolved_destination.starts_with(path.canonicalize()?) {
            bail!("Select files from one folder and save the archive outside selected folders");
        }
    }
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        #[cfg(unix)]
        {
        let mut writer = ArchiveWriter::new(file)?;
        configure_writer(&mut writer, destination)?;
        writer.open()?;
        for path in paths {
            add_path(&mut writer, root, path)?;
        }
        drop(writer);
        }
        #[cfg(not(unix))]
        write_native_archive(file, root, paths, destination)?;
        compress_tools::list_archive_entries(File::open(destination)?)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    crate::ops::refresh_sizes(destination);
    result
}

#[cfg(not(unix))]
fn repack_into(workspace: &Workspace, destination: &Path) -> Result<()> {
    let file = File::options().write(true).create_new(true).open(destination)?;
    write_native_archive(file, &workspace.contents, &[workspace.contents.join(".")], &workspace.source)
}

#[cfg(not(unix))]
fn write_native_archive(mut destination: File, root: &Path, paths: &[PathBuf], format_path: &Path) -> Result<()> {
    use crate::process::CommandOutputExt;
    fn validate(path: &Path) -> Result<()> {
        let metadata=fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || crate::ops::is_reparse_point(&metadata) || (!metadata.is_dir() && !metadata.is_file()) {
            bail!("Cannot archive special file or symlink: {}",path.display());
        }
        if metadata.is_dir() { for item in fs::read_dir(path)? { validate(&item?.path())?; } }
        Ok(())
    }
    for path in paths { validate(path)?; }
    let name=format_path.to_string_lossy().to_ascii_lowercase();
    let (format,filter)=if name.ends_with(".zip") {("zip",None)}
        else if name.ends_with(".7z") {("7zip",None)}
        else if name.ends_with(".tar.gz")||name.ends_with(".tgz") {("pax",Some("--gzip"))}
        else if name.ends_with(".tar.bz2")||name.ends_with(".tbz2") {("pax",Some("--bzip2"))}
        else if name.ends_with(".tar.xz")||name.ends_with(".txz") {("pax",Some("--xz"))}
        else if name.ends_with(".tar.zst")||name.ends_with(".tzst") {("pax",Some("--zstd"))}
        else if name.ends_with(".tar") {("pax",None)} else {bail!("this archive format is read-only")};
    let temporary=tempfile::NamedTempFile::new()?.into_temp_path();
    let mut command=std::process::Command::new("tar.exe");
    command.args(["--create","--format",format,"--file"]).arg(&temporary);
    if let Some(filter)=filter {command.arg(filter);}
    command.arg("--directory").arg(root).arg("--");
    for path in paths { let relative=path.strip_prefix(root)?; command.arg(if relative.as_os_str().is_empty() {Path::new(".")} else {relative});}
    let output=command.bounded_output(Duration::from_secs(1800))?;
    if !output.status.success() {bail!("Archive creation failed: {}",String::from_utf8_lossy(&output.stderr));}
    compress_tools::list_archive_entries(File::open(&temporary)?)?;
    std::io::copy(&mut File::open(&temporary)?, &mut destination)?;
    destination.sync_all()?;
    Ok(())
}

/// Extract into a newly created directory; never merge with existing files.
pub fn extract(source: &Path, destination: &Path) -> Result<()> {
    let size = source.metadata()?.len();
    fs::create_dir(destination)?;
    if let Err(error) = unpack_into(source, destination, size) {
        let _ = fs::remove_dir_all(destination);
        return Err(error);
    }
    crate::ops::refresh_sizes(destination);
    Ok(())
}

#[cfg(unix)]
fn configure_writer(writer: &mut ArchiveWriter<File>, source: &Path) -> Result<()> {
    use simple_archive::{ARCHIVE_FILTER_BZIP2, ARCHIVE_FILTER_NONE, ARCHIVE_FORMAT_TAR};

    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".zip") {
        writer.set_output_zip()?;
    } else if name.ends_with(".7z") {
        writer.set_output_7zlzma2()?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        writer.set_output_targz()?;
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") {
        writer.set_output_format(ARCHIVE_FORMAT_TAR)?;
        writer.set_output_filter(ARCHIVE_FILTER_BZIP2)?;
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        writer.set_output_tarxz()?;
    } else if name.ends_with(".tar.zst") || name.ends_with(".tzst") {
        writer.set_output_tarzst()?;
    } else if name.ends_with(".tar") {
        writer.set_output_format(ARCHIVE_FORMAT_TAR)?;
        writer.set_output_filter(ARCHIVE_FILTER_NONE)?;
    } else {
        bail!("this archive format is read-only")
    }
    Ok(())
}

#[cfg(unix)]
fn add_tree(writer: &mut ArchiveWriter<File>, root: &Path, directory: &Path) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        add_path(writer, root, &path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn add_path(writer: &mut ArchiveWriter<File>, root: &Path, path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || crate::ops::is_reparse_point(&metadata) || (!metadata.is_dir() && !metadata.is_file()) {
        bail!("Cannot archive special file or symlink: {}", path.display());
    }
    let relative = path.strip_prefix(root)?;
    let member = relative
        .to_str()
        .context("archive member name is not valid UTF-8")?
        .replace(std::path::MAIN_SEPARATOR, "/");
    if metadata.is_dir() {
        writer.add_data(&format!("{member}/"), &[], 0, 0, true)?;
        add_tree(writer, root, path)?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};

            let mode = metadata.permissions().mode() & 0o777;
            let meta = simple_archive::Metadata::from_fields(
                metadata.len() as i64,
                simple_archive::AE_IFREG,
                mode,
                metadata.mtime(),
                metadata.mtime_nsec(),
            );
            writer.add_obj_from_reader(File::open(path)?, &member, &meta)?;
        }
        #[cfg(not(unix))]
        writer.add_file(
            path.to_str().context("file path is not valid UTF-8")?,
            &member,
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_source(path: &Path, source: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    fs::write(path, source.as_os_str().as_bytes())?;
    Ok(())
}

#[cfg(not(unix))]
fn write_source(path: &Path, source: &Path) -> Result<()> {
    fs::write(path, source.to_string_lossy().as_bytes())?;
    Ok(())
}

#[cfg(unix)]
fn read_source(path: &Path) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(fs::read(path)?)))
}

#[cfg(not(unix))]
fn read_source(path: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(String::from_utf8(fs::read(path)?)?))
}

fn unpack_into(source: &Path, destination: &Path, compressed_size: u64) -> Result<()> {
    let limit = compressed_size
        .saturating_mul(100)
        .clamp(1 << 30, 100 << 30);
    let mut total = 0_u64;
    let mut output: Option<(File, u32)> = None;
    let mut archive = ArchiveIterator::from_read(File::open(source)?)?;
    for item in &mut archive {
        match item {
            ArchiveContents::StartOfEntry(name, stat) => {
                if matches!(name.as_str(), "." | "./" | ".\\") {
                    output = None;
                    continue;
                }
                let relative = safe_member(&name)?;
                let mode = stat.st_mode as u32;
                let kind = mode & 0o170000;
                if kind == 0o120000 || (kind != 0 && kind != 0o040000 && kind != 0o100000) {
                    output = None;
                    continue;
                }
                let target = destination.join(relative);
                if kind == 0o040000 || name.ends_with('/') || name.ends_with('\\') {
                    fs::create_dir_all(target)?;
                    output = None;
                } else {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    output = Some((File::create(target)?, mode));
                }
            }
            ArchiveContents::DataChunk(data) => {
                total = total.saturating_add(data.len() as u64);
                if total > limit {
                    bail!("archive expands beyond the {limit}-byte safety limit");
                }
                if let Some((file, _)) = output.as_mut() {
                    file.write_all(&data)?;
                }
            }
            ArchiveContents::EndOfEntry => {
                if let Some((_file, _mode)) = output.take() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        _file.set_permissions(fs::Permissions::from_mode(_mode & 0o777))?;
                    }
                }
            }
            ArchiveContents::Err(error) => return Err(error.into()),
        }
    }
    archive.close()?;
    Ok(())
}

fn safe_member(name: &str) -> Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':')
    {
        bail!("archive contains an unsafe path: {name}")
    }
    let mut path = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => {
                #[cfg(windows)]
                {
                    let name = part.to_string_lossy();
                    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                    if name.contains(':') || name.ends_with(['.', ' '])
                        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4
                            && matches!(stem.as_bytes()[3], b'1'..=b'9')) {
                        bail!("archive contains an unsafe Windows filename: {name}");
                    }
                }
                path.push(part);
            },
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("archive contains an unsafe path: {name}")
            }
        }
    }
    if path.as_os_str().is_empty() {
        bail!("archive contains an empty path")
    }
    Ok(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Workspace, configure_writer, repack, safe_member, unpack, workspace};
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, SystemTime};

    #[cfg(unix)]
use simple_archive::writer::ArchiveWriter;
    use tempfile::tempdir;

    #[test]
    fn archive_members_cannot_escape_the_cache() {
        assert_eq!(
            safe_member("folder/file.txt").unwrap(),
            Path::new("folder/file.txt")
        );
        assert!(safe_member("../escape").is_err());
        assert!(safe_member("folder/../../escape").is_err());
        assert!(safe_member("/absolute").is_err());
        assert!(safe_member("C:\\absolute").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn libarchive_unpacks_files_for_direct_opening() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let input = temp.path().join("input");
        fs::create_dir(&input).unwrap();
        let script = input.join("run.sh");
        fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let archive = temp.path().join("sample.tar");
        assert!(
            Command::new("tar")
                .args(["-cf"])
                .arg(&archive)
                .arg("-C")
                .arg(&input)
                .arg(".")
                .status()
                .unwrap()
                .success()
        );

        let output = unpack(&archive).unwrap();
        let extracted = output.join("run.sh");
        assert_eq!(fs::read(&extracted).unwrap(), b"#!/bin/sh\nexit 0\n");
        assert_ne!(
            extracted.metadata().unwrap().permissions().mode() & 0o111,
            0
        );
        assert_eq!(workspace(&output).unwrap().source, archive);
        fs::remove_dir_all(output.parent().unwrap()).unwrap();
    }

    #[test]
    fn writable_formats_round_trip_extracted_changes() {
        let temp = tempdir().unwrap();
        for name in [
            "sample.zip",
            "sample.7z",
            "sample.tar",
            "sample.tar.gz",
            "sample.tar.bz2",
            "sample.tar.xz",
            "sample.tar.zst",
        ] {
            let source = temp.path().join(name);
            let mut writer = ArchiveWriter::new(fs::File::create(&source).unwrap()).unwrap();
            configure_writer(&mut writer, &source).unwrap();
            writer.open().unwrap();
            writer.add_data("note.txt", b"before", 0, 0, false).unwrap();
            drop(writer);
            let root = temp.path().join(format!("workspace-{name}"));
            let contents = root.join("contents");
            fs::create_dir_all(&contents).unwrap();
            fs::write(contents.join("note.txt"), b"after").unwrap();
            let workspace = Workspace {
                root,
                contents,
                source: source.clone(),
            };

            repack(&workspace).unwrap();

            let mut data = Vec::new();
            compress_tools::uncompress_archive_file(
                fs::File::open(source).unwrap(),
                &mut data,
                "note.txt",
            )
            .unwrap();
            assert_eq!(data, b"after", "failed for {name}");
        }
    }

    #[test]
    fn cache_pruning_keeps_the_open_workspace() {
        let temp = tempdir().unwrap();
        let open = temp.path().join("open");
        let stale = temp.path().join("stale");
        fs::create_dir_all(open.join("contents")).unwrap();
        fs::create_dir_all(&stale).unwrap();
        fs::write(open.join("ready"), []).unwrap();
        fs::write(stale.join("ready"), []).unwrap();

        super::prune_cache_at(
            temp.path(),
            Some(&open.join("contents")),
            SystemTime::now() + super::CACHE_MAX_AGE + Duration::from_secs(1),
        );

        assert_eq!((open.exists(), stale.exists()), (true, false));
    }
}
