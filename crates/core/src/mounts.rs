use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const SKIP_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "tmpfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "squashfs",
    "autofs",
    "fusectl",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "bpf",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "efivarfs",
    "hugetlbfs",
    "mqueue",
    "ramfs",
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "nsfs",
    "devpts",
    "binfmt_misc",
    "configfs",
];

const KEEP_FS: &[&str] = &[
    "ext4", "ext3", "ext2", "btrfs", "xfs", "f2fs", "zfs", "ntfs", "ntfs3", "fuseblk", "vfat",
    "exfat", "jfs", "reiserfs",
];

/// Local Mounts worth putting in the Catalog.
pub(crate) fn discover() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        discover_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        vec![PathBuf::from("/")]
    }
}

#[cfg(target_os = "linux")]
fn discover_linux() -> Vec<PathBuf> {
    let Ok(file) = File::open("/proc/self/mounts") else {
        return vec![PathBuf::from("/")];
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let mut parts = line.split_whitespace();
        let Some(_src) = parts.next() else { continue };
        let Some(target) = parts.next() else { continue };
        let Some(fstype) = parts.next() else { continue };
        if SKIP_FS.contains(&fstype) {
            continue;
        }
        if !KEEP_FS.contains(&fstype) && !fstype.starts_with("fuse.") {
            continue;
        }
        if fstype.starts_with("fuse.") && fstype != "fuseblk" {
            continue;
        }
        let path = PathBuf::from(unescape_mount(target));
        if !path.is_dir() {
            continue;
        }
        out.push(path);
    }
    if out.is_empty() {
        out.push(PathBuf::from("/"));
    }
    out.sort();
    out.dedup();
    out
}

fn unescape_mount(s: &str) -> String {
    s.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

pub(crate) fn is_under_skip_mount(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("/proc" | "/sys" | "/dev" | "/run" | "/snap")
    ) || path.starts_with("/proc")
        || path.starts_with("/sys")
        || path.starts_with("/dev")
        || path.starts_with("/run")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_spaces() {
        assert_eq!(unescape_mount("/mnt/my\\040disk"), "/mnt/my disk");
    }
}
