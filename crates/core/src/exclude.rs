use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::error::{Error, Result};

/// Directory names skipped wherever they appear.
const NAME_EXCLUDES: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "target",
    ".cache",
    ".npm",
    ".cargo",
    ".rustup",
    "lost+found",
    "WinSxS",
    "SysWOW64",
    "System32",
    "$Recycle.Bin",
    "System Volume Information",
];

#[cfg(target_os = "windows")]
const PLATFORM_NAME_EXCLUDES: &[&str] = &[];

#[cfg(target_os = "macos")]
const PLATFORM_NAME_EXCLUDES: &[&str] = &[
    ".DocumentRevisions-V100",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
];

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const PLATFORM_NAME_EXCLUDES: &[&str] = &[];

/// Path globs skipped in addition to [`NAME_EXCLUDES`].
#[cfg(target_os = "linux")]
const PLATFORM_PATH_EXCLUDES: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/tmp",
    "/snap",
    "/var/tmp",
    "/var/cache",
    "/var/lib/docker",
    "/var/lib/containers",
    "/var/lib/flatpak",
    "**/Windows/assembly",
    "**/Windows/Installer",
    "**/Windows/servicing",
    "**/Windows.old",
];

#[cfg(target_os = "macos")]
const PLATFORM_PATH_EXCLUDES: &[&str] = &[
    "/System/Volumes/Data",
    "/private/var/folders",
    "**/Library/Caches",
];

#[cfg(target_os = "windows")]
const PLATFORM_PATH_EXCLUDES: &[&str] = &[
    "**/Windows/assembly",
    "**/Windows/Installer",
    "**/Windows/servicing",
    "**/Windows.old",
];

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const PLATFORM_PATH_EXCLUDES: &[&str] = &[];

#[derive(Clone)]
pub(crate) struct Excludes {
    names: Vec<String>,
    globs: GlobSet,
    paths: Vec<PathBuf>,
}

impl Excludes {
    #[cfg(test)]
    pub(crate) fn new(extra: &[String]) -> Result<Self> {
        Self::with_paths(extra, &[])
    }

    pub(crate) fn with_paths(extra: &[String], paths: &[PathBuf]) -> Result<Self> {
        let mut names: Vec<String> = NAME_EXCLUDES
            .iter()
            .chain(PLATFORM_NAME_EXCLUDES)
            .map(|s| (*s).to_string())
            .collect();
        let mut builder = GlobSetBuilder::new();
        for pat in PLATFORM_PATH_EXCLUDES
            .iter()
            .copied()
            .chain(extra.iter().map(String::as_str))
        {
            if !pat.contains(['/', '*', '?']) {
                names.push(pat.to_string());
                continue;
            }
            let glob = Glob::new(pat).map_err(|source| Error::Exclude {
                pattern: pat.to_string(),
                source,
            })?;
            builder.add(glob);
        }
        let globs = builder.build().map_err(|source| Error::Exclude {
            pattern: "<set>".into(),
            source,
        })?;
        Ok(Self {
            names,
            globs,
            paths: paths.to_vec(),
        })
    }

    pub(crate) fn skip(&self, path: &Path) -> bool {
        if self.globs.is_match(path) || self.paths.iter().any(|root| path.starts_with(root)) {
            return true;
        }
        path.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|n| self.names.iter().any(|ex| ex == n))
        })
    }

    pub(crate) fn skip_name(&self, name: &OsStr) -> bool {
        name.to_str()
            .is_some_and(|n| self.names.iter().any(|ex| ex == n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn skips_node_modules_anywhere() {
        let ex = Excludes::new(&[]).expect("excludes");
        assert!(ex.skip(Path::new("/home/me/src/node_modules/pkg")));
        assert!(ex.skip_name(OsStr::new("node_modules")));
    }

    #[test]
    fn skips_windows_system32() {
        let ex = Excludes::new(&[]).expect("excludes");
        assert!(ex.skip(Path::new("/mnt/Windows11/Windows/System32/cmd.exe")));
    }

    #[test]
    fn extra_name_exclude() {
        let ex = Excludes::new(&["secret".into()]).expect("excludes");
        assert!(ex.skip(Path::new("/home/secret/file")));
    }
}
