//! Live folder queries shared by native and GTK frontends.
use crate::{Config, IgnoreMatcher, MatchMode, Scope, SearchOpts, Sort};
use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

pub struct LiveEntry {
    pub name: String,
    pub sort_name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

fn name_matches(name: &str, words: &[&str], mode: MatchMode) -> bool {
    let name = name.to_lowercase();
    words.iter().all(|word| {
        let word = word.to_lowercase();
        match mode {
            MatchMode::Exact => name == word,
            MatchMode::Substring => name.contains(&word),
            MatchMode::Fuzzy => {
                let mut chars = name.chars();
                word.chars()
                    .all(|wanted| chars.by_ref().any(|got| got == wanted))
            }
        }
    })
}

pub fn live_children(
    path: &Path,
    query: &str,
    opts: SearchOpts,
    folders_first: bool,
    measure_size: bool,
) -> crate::Result<Vec<LiveEntry>> {
    let cfg = Config::load();
    let parsed = crate::search::parse_query(query)?;
    let mut ignored = IgnoreMatcher::new(cfg.respect_gitignore, cfg.respect_ignore);
    let entries = std::fs::read_dir(path).map_err(|error| crate::Error::io(path, error))?;
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !cfg.show_hidden && name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_dir = file_type.is_dir() || (file_type.is_symlink() && path.is_dir());
        if ignored
            .as_mut()
            .is_some_and(|matcher| matcher.is_ignored(&path, is_dir))
            || !match opts.scope {
                Scope::All => true,
                Scope::Files => !is_dir,
                Scope::Folders => is_dir,
            }
            || !opts.class.matches(&name, is_dir)
            || (is_dir && !parsed.exts.is_empty())
            || !crate::search::name_passes(&name, &parsed.globs, &parsed.exts)
            || !name_matches(&name, &parsed.fuzzy, opts.match_mode)
        {
            continue;
        }
        let (size, mtime) = if opts.sort.needs_stat() || measure_size {
            entry.metadata().map_or((0, 0), |meta| {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |age| age.as_secs() as i64);
                (meta.len(), mtime)
            })
        } else {
            (0, 0)
        };
        rows.push(LiveEntry {
            sort_name: name.to_lowercase(),
            name,
            path,
            is_dir,
            size,
            mtime,
        });
    }
    rows.sort_by(|a, b| {
        let folder_order = b.is_dir.cmp(&a.is_dir);
        if folders_first && folder_order != Ordering::Equal {
            return folder_order;
        }
        let name = || a.sort_name.cmp(&b.sort_name);
        match opts.sort {
            Sort::NameDesc => name().reverse(),
            Sort::Newest => b.mtime.cmp(&a.mtime).then_with(name),
            Sort::Oldest => a.mtime.cmp(&b.mtime).then_with(name),
            Sort::Largest => b.size.cmp(&a.size).then_with(name),
            Sort::Smallest => a.size.cmp(&b.size).then_with(name),
            Sort::Score | Sort::Name => name(),
        }
    });
    if opts.limit > 0 {
        rows.truncate(opts.limit);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_query_keeps_ext_tokens_and_spaces() {
        let dir = std::env::temp_dir().join(format!("qfind-live-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["shot.png", "Art.PNG", "art.psd", "notes.txt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let names = |q: &str| {
            let mut v: Vec<String> = live_children(&dir, q, SearchOpts::default(), true, false)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(".png"), ["Art.PNG", "shot.png"]);
        assert_eq!(names("art .png"), ["Art.PNG"]);
        assert_eq!(names("art .png .psd"), ["Art.PNG", "art.psd"]);
        assert_eq!(names("*.txt"), ["notes.txt"]);
        assert_eq!(names("").len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
