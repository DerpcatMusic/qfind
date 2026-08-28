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

    pub(crate) fn name(&self, entry: Entry) -> &str {
        let start = self.names_off + entry.name_off as usize;
        let end = start + entry.name_len as usize;
        std::str::from_utf8(self.bytes.get(start..end).unwrap_or(b"")).unwrap_or("")
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
                    *slot = prefilter::mask_name(self.name(e).as_bytes());
                }
            }
            let _ = write_mask_file(&self.path.with_extension("mask"), &v);
            v.into_boxed_slice()
        })
    }

    fn mapped_masks(&self) -> Option<&[u64]> {
        let map = self.mask_map.as_ref()?;
        mask_slice(map, self.len() as usize)
    }

    pub(crate) fn path(&self, id: u32) -> PathBuf {
        let mut parts = Vec::with_capacity(16);
        let mut cur = id;
        let mut guard = 0u32;
        while let Some(e) = self.entry(cur) {
            parts.push(self.name(e));
            if e.parent == Entry::ROOT_PARENT {
                break;
            }
            cur = e.parent;
            guard += 1;
            if guard > 512 {
                break;
            }
        }
        parts.reverse();
        if parts.is_empty() {
            return PathBuf::from(".");
        }
        let mut path = PathBuf::from(parts[0]);
        for p in &parts[1..] {
            path.push(p);
        }
        path
    }
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

    pub(crate) fn intern_dir(&mut self, path: &Path, walk_root: &Path) -> u32 {
        if let Some(id) = self.ids.get(path) {
            return id;
        }
        if path == walk_root {
            return self.push_root(walk_root);
        }
        let parent_path = path.parent().unwrap_or(walk_root);
        let parent = self.intern_dir(parent_path, walk_root);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        self.push_dir(path.to_path_buf(), parent, name, 0, 0)
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
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let (name_off, name_len) = self.push_name(name);
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
        let name = walk_root.to_string_lossy();
        self.push_dir(
            walk_root.to_path_buf(),
            Entry::ROOT_PARENT,
            name.as_ref(),
            0,
            0,
        )
    }

    fn push_dir(&mut self, path: PathBuf, parent: u32, name: &str, size: u64, mtime: i64) -> u32 {
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

    fn push_name(&mut self, name: &str) -> (u32, u32) {
        let off = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        let bytes = name.as_bytes();
        self.names.extend_from_slice(bytes);
        (off, u32::try_from(bytes.len()).unwrap_or(u32::MAX))
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let tmp = path.with_extension("tmp");
        {
            let file = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            let mut w = BufWriter::new(file);
            self.write_to(&mut w).map_err(|e| Error::io(&tmp, e))?;
            w.flush().map_err(|e| Error::io(&tmp, e))?;
        }
        fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
        let mut masks = Vec::with_capacity(self.folders.len() + self.files.len());
        for e in self.folders.iter().chain(self.files.iter()) {
            let start = e.name_off as usize;
            let end = start.saturating_add(e.name_len as usize);
            let name = self.names.get(start..end).unwrap_or(b"");
            masks.push(prefilter::mask_name(name));
        }
        let _ = write_mask_file(&path.with_extension("mask"), &masks);
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
    if data.as_ptr() as usize % 8 != 0 {
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
