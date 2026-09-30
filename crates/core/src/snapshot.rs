use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::prefilter;

pub(crate) const MAGIC: &[u8; 4] = b"QFND";
pub(crate) const VERSION: u32 = 1;
pub(crate) const ENTRY_SIZE: usize = 32;
const HEADER_SIZE: usize = 28;

/// Hard ceiling on how far up the parent chain we will walk. Real trees are
/// nowhere near this; hitting it means the snapshot is malformed, and every
/// ancestor walk shares this bound so they can never disagree about a path.
const MAX_DEPTH: u32 = 512;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    pub parent: u32,
    pub name_off: u32,
    pub name_len: u32,
    pub flags: u32,
    pub size: u64,
    pub mtime: i64,
}

impl Entry {
    pub(crate) const ROOT_PARENT: u32 = u32::MAX;
    pub(crate) const DIR: u32 = 1;

    pub(crate) fn is_dir(self) -> bool {
        self.flags & Self::DIR != 0
    }
}

pub(crate) struct Snapshot {
    path: PathBuf,
    bytes: Mmap,
    mask_map: Option<Mmap>,
    folder_count: u32,
    file_count: u32,
    names_off: usize,
    letter_mask: OnceLock<Box<[u64]>>,
    hidden: OnceLock<Box<[bool]>>,
    folder_paths: OnceLock<FolderIndex>,
}

/// Exact path → folder id, plus a case-folded tier for the case-insensitive
/// filesystems where `/Users/me` and `/users/me` are the same folder.
struct FolderIndex {
    exact: std::collections::HashMap<PathBuf, u32>,
    folded: std::collections::HashMap<String, u32>,
}

impl FolderIndex {
    fn get(&self, path: &Path) -> Option<u32> {
        self.exact.get(path).copied().or_else(|| {
            cfg!(any(windows, target_os = "macos"))
                .then(|| self.folded.get(&fold_key(path)).copied())
                .flatten()
        })
    }
}

/// Case-folded lookup key. Only consulted after an exact miss, so a
/// case-sensitive filesystem is unaffected.
#[cfg(any(windows, target_os = "macos"))]
fn fold_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// On case-sensitive filesystems an exact miss is a genuine miss.
#[cfg(not(any(windows, target_os = "macos")))]
fn fold_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Snapshot {
    pub(crate) fn open_mmap(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        // SAFETY: snapshot is immutable. Rebuild writes a temp file then rename,
        // so this inode stays valid for the lifetime of the map.
        let bytes = unsafe { Mmap::map(&file).map_err(|e| Error::io(path, e))? };
        Self::parse(path, bytes)
    }

    fn parse(path: &Path, bytes: Mmap) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::Snapshot {
                path: path.to_path_buf(),
                reason: "truncated header",
            });
        }
        if &bytes[0..4] != MAGIC {
            return Err(Error::Snapshot {
                path: path.to_path_buf(),
                reason: "bad magic",
            });
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
        if version != VERSION {
            return Err(Error::Snapshot {
                path: path.to_path_buf(),
                reason: "unsupported version",
            });
        }
        let folder_count = u32::from_le_bytes(bytes[12..16].try_into().expect("4 bytes"));
        let file_count = u32::from_le_bytes(bytes[16..20].try_into().expect("4 bytes"));
        let names_len = u32::from_le_bytes(bytes[20..24].try_into().expect("4 bytes")) as usize;
        let names_off =
            HEADER_SIZE + (folder_count as usize + file_count as usize).saturating_mul(ENTRY_SIZE);
        let need = names_off.saturating_add(names_len);
        if bytes.len() < need {
            return Err(Error::Snapshot {
                path: path.to_path_buf(),
                reason: "truncated body",
            });
        }
        let mask_map = mmap_mask(&path.with_extension("mask"), folder_count + file_count);
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            mask_map,
            folder_count,
            file_count,
            names_off,
            letter_mask: OnceLock::new(),
            hidden: OnceLock::new(),
            folder_paths: OnceLock::new(),
        })
    }

    pub(crate) fn folder_count(&self) -> u32 {
        self.folder_count
    }

    pub(crate) fn file_count(&self) -> u32 {
        self.file_count
    }

    pub(crate) fn len(&self) -> u32 {
        self.folder_count.saturating_add(self.file_count)
    }

    pub(crate) fn entry(&self, id: u32) -> Option<Entry> {
        if id >= self.len() {
            return None;
        }
        let off = HEADER_SIZE + id as usize * ENTRY_SIZE;
        let b = &self.bytes[off..off + ENTRY_SIZE];
        Some(Entry {
            parent: u32::from_le_bytes(b[0..4].try_into().ok()?),
            name_off: u32::from_le_bytes(b[4..8].try_into().ok()?),
            name_len: u32::from_le_bytes(b[8..12].try_into().ok()?),
            flags: u32::from_le_bytes(b[12..16].try_into().ok()?),
            size: u64::from_le_bytes(b[16..24].try_into().ok()?),
            mtime: i64::from_le_bytes(b[24..32].try_into().ok()?),
        })
    }

    pub(crate) fn names_bytes(&self) -> &[u8] {
        &self.bytes[self.names_off..]
    }

    /// Walk parents up to the Mount root this entry was walked from.
    pub(crate) fn root_of(&self, id: u32) -> u32 {
        self.ancestors(id).last().map_or(id, |(id, _)| id)
    }

    /// Raw on-disk bytes of an entry's name. Names are *not* guaranteed UTF-8;
    /// a latin-1 or GBK filename is stored verbatim so [`Self::path`] can hand
    /// back the exact path the filesystem has.
    pub(crate) fn name_bytes(&self, entry: Entry) -> &[u8] {
        let start = self.names_off.saturating_add(entry.name_off as usize);
        let end = start.saturating_add(entry.name_len as usize);
        self.bytes.get(start..end).unwrap_or_default()
    }

    /// The entry's name for display and matching. Borrows when the name is
    /// valid UTF-8 (the overwhelming majority) and only allocates for the rest.
    pub(crate) fn name(&self, entry: Entry) -> std::borrow::Cow<'_, str> {
        let raw = self.name_bytes(entry);
        String::from_utf8_lossy(raw)
    }

    /// One `u64` letter/digit mask per id. Prefers the sidecar mmap written on Rebuild.
    pub(crate) fn letter_mask(&self) -> &[u64] {
        if let Some(mapped) = self.mapped_masks() {
            return mapped;
        }
        self.letter_mask.get_or_init(|| {
            let n = self.len() as usize;
            let mut v = vec![0u64; n];
            for (id, slot) in v.iter_mut().enumerate() {
                if let Some(e) = self.entry(id as u32) {
                    *slot = prefilter::mask_name(self.name_bytes(e));
                }
            }
            let _ = write_mask_file(&self.mask_path(), &v);
            v.into_boxed_slice()
        })
    }

    fn mask_path(&self) -> PathBuf {
        self.path.with_extension("mask")
    }

    fn mapped_masks(&self) -> Option<&[u64]> {
        let map = self.mask_map.as_ref()?;
        mask_slice(map, self.len() as usize)
    }

    /// Ancestors of `id`, nearest first, ending at its Mount root.
    ///
    /// Every parent walk in this file goes through here so the depth cap, the
    /// root test, and the parent ordering can never drift apart.
    fn ancestors(&self, id: u32) -> Ancestors<'_> {
        Ancestors {
            snapshot: self,
            next: Some(id),
            depth: 0,
        }
    }

    pub(crate) fn path(&self, id: u32) -> PathBuf {
        let mut parts: Vec<&[u8]> = self
            .ancestors(id)
            .map(|(_, e)| self.name_bytes(e))
            .collect();
        parts.reverse();
        let Some((first, rest)) = parts.split_first() else {
            return PathBuf::from(".");
        };
        let mut path = os_path(first);
        for part in rest {
            path.push(os_path(part));
        }
        path
    }

    pub(crate) fn folder_id(&self, path: &Path) -> Option<u32> {
        self.folder_paths
            .get_or_init(|| {
                let mut index = FolderIndex {
                    exact: std::collections::HashMap::with_capacity(self.folder_count as usize),
                    folded: std::collections::HashMap::new(),
                };
                for id in 0..self.folder_count {
                    let path = self.path(id);
                    // Case-sensitive filesystems never consult `folded`, so
                    // do not copy every Folder path into it.
                    if cfg!(any(windows, target_os = "macos")) {
                        index.folded.entry(fold_key(&path)).or_insert(id);
                    }
                    index.exact.entry(path).or_insert(id);
                }
                index
            })
            .get(path)
    }

    pub(crate) fn is_descendant_of(&self, id: u32, folder: u32) -> bool {
        id != folder && self.ancestors(id).any(|(id, _)| id == folder)
    }

    pub(crate) fn is_hidden(&self, id: u32) -> bool {
        self.hidden
            .get_or_init(|| {
                let mut hidden = vec![false; self.len() as usize];
                for current in 0..self.len() {
                    let Some(entry) = self.entry(current) else {
                        continue;
                    };
                    let name = self.name_bytes(entry);
                    let own = if entry.parent == Entry::ROOT_PARENT {
                        os_path(name)
                            .components()
                            .any(|part| part.as_os_str().as_encoded_bytes().first() == Some(&b'.'))
                    } else {
                        name.first() == Some(&b'.')
                    };
                    let inherited = entry
                        .parent
                        .try_into()
                        .ok()
                        .and_then(|parent: usize| hidden.get(parent))
                        .copied()
                        .unwrap_or(false);
                    hidden[current as usize] = own || inherited;
                }
                hidden.into_boxed_slice()
            })
            .get(id as usize)
            .copied()
            .unwrap_or(false)
    }
}

/// `id` plus each ancestor's entry, nearest first, stopping at the Mount root
/// or at [`MAX_DEPTH`].
struct Ancestors<'a> {
    snapshot: &'a Snapshot,
    next: Option<u32>,
    depth: u32,
}

impl Iterator for Ancestors<'_> {
    type Item = (u32, Entry);

    fn next(&mut self) -> Option<Self::Item> {
        if self.depth > MAX_DEPTH {
            return None;
        }
        let id = self.next?;
        let entry = self.snapshot.entry(id)?;
        self.next = (entry.parent != Entry::ROOT_PARENT).then_some(entry.parent);
        self.depth += 1;
        Some((id, entry))
    }
}

/// Rebuild a path component from raw bytes. `OsStr::from_bytes` is exact on
/// Unix, so a non-UTF-8 name round-trips instead of collapsing to its parent.
fn os_path(raw: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(raw))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(raw).into_owned())
    }
}

/// Raw bytes of a path's final component. Never lossy and never empty, so an
/// entry can always be turned back into the exact path the filesystem has.
#[cfg(unix)]
fn name_bytes(path: &Path) -> &[u8] {
    use std::ffi::OsStr;
    path.file_name().map_or(b"", OsStr::as_encoded_bytes)
}

/// Non-Unix filesystems hand out UTF-8, so the lossy form is already exact.
#[cfg(not(unix))]
fn name_bytes(path: &Path) -> &[u8] {
    path.file_name()
        .map_or(b"", |name| name.to_string_lossy().as_bytes())
}

/// Raw bytes of a whole path, used for a Mount root's own name.
#[cfg(unix)]
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> &[u8] {
    path.to_string_lossy().as_bytes()
}

pub(crate) struct Builder {
    folders: Vec<Entry>,
    files: Vec<Entry>,
    ids: rustc_hash_map::PathMap,
    names: Vec<u8>,
}

/// Tiny path intern table. Kept here so snapshot does not depend on a hash crate.
mod rustc_hash_map {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    pub(crate) struct PathMap(HashMap<PathBuf, u32>);

    impl PathMap {
        pub(crate) fn new() -> Self {
            Self(HashMap::new())
        }

        pub(crate) fn get(&self, path: &Path) -> Option<u32> {
            self.0.get(path).copied()
        }

        pub(crate) fn insert(&mut self, path: PathBuf, id: u32) {
            self.0.insert(path, id);
        }
    }
}

impl Builder {
    pub(crate) fn new() -> Self {
        Self {
            folders: Vec::new(),
            files: Vec::new(),
            ids: rustc_hash_map::PathMap::new(),
            names: Vec::new(),
        }
    }

    /// Copy every entry of `snap` except `skip` and its descendants, keeping
    /// the name blob verbatim so a copy is one Vec push per entry. Returns the
    /// old-folder-id → new-id table (`u32::MAX` = dropped).
    pub(crate) fn from_snapshot(snap: &Snapshot, skip: Option<u32>) -> (Self, Vec<u32>) {
        let n_folders = snap.folder_count();
        let mut b = Self {
            folders: Vec::with_capacity(n_folders as usize),
            files: Vec::with_capacity(snap.file_count() as usize),
            ids: rustc_hash_map::PathMap::new(),
            names: snap.names_bytes().to_vec(),
        };
        let mut new_ids = vec![u32::MAX; n_folders as usize];
        for id in 0..n_folders {
            let Some(e) = snap.entry(id) else { continue };
            if Some(id) == skip {
                continue;
            }
            let parent = if e.parent == Entry::ROOT_PARENT {
                Entry::ROOT_PARENT
            } else {
                // Parents always precede children, so a dropped parent is already known.
                match new_ids.get(e.parent as usize) {
                    Some(&p) if p != u32::MAX => p,
                    _ => continue,
                }
            };
            new_ids[id as usize] = u32::try_from(b.folders.len()).unwrap_or(u32::MAX);
            b.folders.push(Entry { parent, ..e });
        }
        for id in n_folders..snap.len() {
            let Some(e) = snap.entry(id) else { continue };
            match new_ids.get(e.parent as usize) {
                Some(&parent) if parent != u32::MAX => b.files.push(Entry { parent, ..e }),
                _ => {}
            }
        }
        (b, new_ids)
    }

    /// Make a copied folder reachable by path so a later walk can hang under it.
    pub(crate) fn register_dir(&mut self, path: PathBuf, id: u32) {
        self.ids.insert(path, id);
    }

    pub(crate) fn intern_dir(&mut self, path: &Path, walk_root: &Path) -> u32 {
        if let Some(id) = self.ids.get(path) {
            return id;
        }
        if path == walk_root {
            return self.push_root(walk_root);
        }
        let parent_path = path.parent().unwrap_or(walk_root);
        let parent = self.intern_dir(parent_path, walk_root);
        self.push_dir(path.to_path_buf(), parent, name_bytes(path), 0, 0)
    }

    pub(crate) fn add_dir(&mut self, path: &Path, walk_root: &Path, size: u64, mtime: i64) -> u32 {
        let id = self.intern_dir(path, walk_root);
        if let Some(e) = self.folders.get_mut(id as usize) {
            e.size = size;
            e.mtime = mtime;
        }
        id
    }

    pub(crate) fn add_file(&mut self, path: &Path, walk_root: &Path, size: u64, mtime: i64) {
        let parent_path = path.parent().unwrap_or(walk_root);
        let parent = self.intern_dir(parent_path, walk_root);
        let (name_off, name_len) = self.push_name(name_bytes(path));
        self.files.push(Entry {
            parent,
            name_off,
            name_len,
            flags: 0,
            size,
            mtime,
        });
    }

    fn push_root(&mut self, walk_root: &Path) -> u32 {
        if let Some(id) = self.ids.get(walk_root) {
            return id;
        }
        // A Mount root is stored under its *whole* path, so `path(id)` on any
        // entry below it comes back absolute rather than root-relative.
        self.push_dir(
            walk_root.to_path_buf(),
            Entry::ROOT_PARENT,
            path_bytes(walk_root),
            0,
            0,
        )
    }

    fn push_dir(&mut self, path: PathBuf, parent: u32, name: &[u8], size: u64, mtime: i64) -> u32 {
        let (name_off, name_len) = self.push_name(name);
        let id = u32::try_from(self.folders.len()).unwrap_or(u32::MAX);
        self.folders.push(Entry {
            parent,
            name_off,
            name_len,
            flags: Entry::DIR,
            size,
            mtime,
        });
        self.ids.insert(path, id);
        id
    }

    fn push_name(&mut self, name: &[u8]) -> (u32, u32) {
        let off = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.extend_from_slice(name);
        (off, u32::try_from(name.len()).unwrap_or(u32::MAX))
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        // The mask sidecar must land *before* the snapshot it describes. A
        // reader that opens in between then sees a complete pair, never a new
        // entry table paired with the previous permutation of masks.
        let mut masks = Vec::with_capacity(self.folders.len() + self.files.len());
        for e in self.folders.iter().chain(self.files.iter()) {
            let start = e.name_off as usize;
            let end = start.saturating_add(e.name_len as usize);
            masks.push(prefilter::mask_name(
                self.names.get(start..end).unwrap_or(b""),
            ));
        }
        let _ = write_mask_file(&path.with_extension("mask"), &masks);

        let tmp = path.with_extension("tmp");
        {
            let file = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            let mut w = BufWriter::new(file);
            self.write_to(&mut w).map_err(|e| Error::io(&tmp, e))?;
            w.flush().map_err(|e| Error::io(&tmp, e))?;
        }
        fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
        Ok(())
    }

    fn write_to<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let folder_count = u32::try_from(self.folders.len()).unwrap_or(u32::MAX);
        let file_count = u32::try_from(self.files.len()).unwrap_or(u32::MAX);
        let names_len = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
        w.write_all(&folder_count.to_le_bytes())?;
        w.write_all(&file_count.to_le_bytes())?;
        w.write_all(&names_len.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
        for e in self.folders.iter().chain(self.files.iter()) {
            write_entry(w, *e)?;
        }
        w.write_all(&self.names)?;
        Ok(())
    }
}

const MASK_MAGIC: &[u8; 4] = b"QMSK";

fn mmap_mask(path: &Path, n: u32) -> Option<Mmap> {
    let file = File::open(path).ok()?;
    // SAFETY: sidecar is rewritten next to the snapshot via temp+rename on Rebuild.
    let map = unsafe { Mmap::map(&file).ok()? };
    if mask_slice(&map, n as usize).is_some() {
        Some(map)
    } else {
        None
    }
}

fn mask_slice(bytes: &[u8], n: usize) -> Option<&[u64]> {
    if bytes.len() != 8 + n * 8 || &bytes[..4] != MASK_MAGIC {
        return None;
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if count as usize != n {
        return None;
    }
    let data = &bytes[8..];
    if !(data.as_ptr() as usize).is_multiple_of(8) {
        return None;
    }
    // SAFETY: length is n * 8, pointer 8-aligned, sidecar is immutable for this inode.
    Some(unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u64>(), n) })
}

fn write_mask_file(path: &Path, masks: &[u64]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(8 + masks.len() * 8);
    buf.extend_from_slice(MASK_MAGIC);
    buf.extend_from_slice(&(masks.len() as u32).to_le_bytes());
    for m in masks {
        buf.extend_from_slice(&m.to_le_bytes());
    }
    let tmp = path.with_extension("masktmp");
    fs::write(&tmp, &buf)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn write_entry<W: Write>(w: &mut W, e: Entry) -> std::io::Result<()> {
    w.write_all(&e.parent.to_le_bytes())?;
    w.write_all(&e.name_off.to_le_bytes())?;
    w.write_all(&e.name_len.to_le_bytes())?;
    w.write_all(&e.flags.to_le_bytes())?;
    w.write_all(&e.size.to_le_bytes())?;
    w.write_all(&e.mtime.to_le_bytes())?;
    Ok(())
}
