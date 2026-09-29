use std::cell::RefCell;

use globset::GlobBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::prefilter;
use crate::query::{MatchMode, Scope, SearchOpts, Sort, class_matches, date_cutoff, date_matches};
use crate::snapshot::{Entry, Snapshot};

pub(crate) struct Ranked {
    pub ids: Vec<u32>,
    pub indices: Vec<Vec<u32>>,
}

/// A scored entry: `(score, id)`. Aliased so sort comparators stay readable.
type Scored = (u32, u32);

pub(crate) fn search_with_cancel(
    snapshot: &Snapshot,
    query: &str,
    opts: SearchOpts,
    folder: Option<u32>,
    direct_children: bool,
    cancelled: &(impl Fn() -> bool + Sync),
) -> Result<Ranked> {
    let show_hidden = opts.show_hidden;
    let parsed = parse_query(query)?;
    if parsed.fuzzy.is_empty() {
        return scan_filtered(snapshot, opts, folder, direct_children, cancelled, &parsed);
    }

    let Parsed {
        fuzzy: fuzzy_parts,
        exts,
        ..
    } = &parsed;
    let pattern = compile_pattern(&fuzzy_parts.join(" "), opts.match_mode);
    let cutoff = date_cutoff(opts.date);
    let keep = |id: u32| -> Option<std::borrow::Cow<'_, str>> {
        let entry = snapshot.entry(id)?;
        if let Some(folder) = folder {
            let belongs = if direct_children {
                entry.parent == folder
            } else {
                snapshot.is_descendant_of(id, folder)
            };
            if !belongs {
                return None;
            }
        }
        if !show_hidden && snapshot.is_hidden(id) {
            return None;
        }
        if !exts.is_empty() && entry.is_dir() {
            return None;
        }
        if !date_matches(opts.date, entry.mtime, cutoff) {
            return None;
        }
        let name = snapshot.name(entry);
        let name = name.as_ref();
        if !class_matches(opts.class, name, entry.is_dir()) {
            return None;
        }
        if !name_passes(name, &parsed.globs, exts) {
            return None;
        }
        Some(snapshot.name(entry))
    };

    let need = prefilter::needle_mask(fuzzy_parts);
    let masks = snapshot.letter_mask();
    let files_only = !exts.is_empty() || opts.scope == Scope::Files;
    let (slice, base) = if files_only {
        let start = snapshot.folder_count() as usize;
        let start = start.min(masks.len());
        (&masks[start..], start as u32)
    } else if opts.scope == Scope::Folders {
        let n = snapshot.folder_count() as usize;
        (&masks[..n.min(masks.len())], 0u32)
    } else {
        (masks, 0u32)
    };
    let mut cands = Vec::new();
    prefilter::scan_mask(slice, need, base, &mut cands);
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let score_one = |id: u32| {
        if cancelled() {
            return None;
        }
        let name = keep(id)?;
        score_only(&pattern, &name).map(|score| (score, id))
    };
    let mut scored: Vec<(u32, u32)> = if cands.len() < 4096 {
        cands.into_iter().filter_map(score_one).collect()
    } else {
        cands.into_par_iter().filter_map(score_one).collect()
    };

    if cancelled() {
        return Err(Error::Cancelled);
    }

    apply_sort(snapshot, &mut scored, opts, cancelled)?;
    if opts.limit > 0 && scored.len() > opts.limit {
        scored.truncate(opts.limit);
    }

    if !opts.highlight {
        return Ok(take_ids(scored));
    }

    let mut ids = Vec::with_capacity(scored.len());
    let mut indices = Vec::with_capacity(scored.len());
    for (_, id) in scored {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let idx = snapshot
            .entry(id)
            .and_then(|e| highlight(&pattern, &snapshot.name(e)))
            .unwrap_or_default();
        indices.push(idx);
        ids.push(id);
    }
    Ok(Ranked { ids, indices })
}

fn compile_pattern(text: &str, mode: MatchMode) -> Pattern {
    match mode {
        MatchMode::Fuzzy => Pattern::parse(text, CaseMatching::Ignore, Normalization::Smart),
        MatchMode::Substring => Pattern::new(
            text,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Substring,
        ),
        MatchMode::Exact => Pattern::new(
            text,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Exact,
        ),
    }
}

fn scan_filtered(
    snapshot: &Snapshot,
    opts: SearchOpts,
    folder: Option<u32>,
    direct_children: bool,
    cancelled: &(impl Fn() -> bool + Sync),
    parsed: &Parsed<'_>,
) -> Result<Ranked> {
    let show_hidden = opts.show_hidden;
    let cutoff = date_cutoff(opts.date);
    // A cap here would decide *which* entries exist, so it may only apply where
    // every surviving entry is equally good: an unordered Score over a whole
    // subtree. A single folder's children are bounded by the filesystem, and
    // every other Sort reads its ordering straight out of the snapshot, so
    // neither needs one.
    let has_ext_filter = !parsed.exts.is_empty();
    let cap = match (opts.sort, direct_children) {
        (Sort::Score, false) if opts.limit > 0 => opts.limit,
        _ => usize::MAX,
    };
    let mut scored = Vec::new();
    if cap < usize::MAX {
        scored.reserve(cap);
    }
    let push_range = |start: u32, end: u32, out: &mut Vec<(u32, u32)>| {
        for id in start..end {
            if cancelled() {
                break;
            }
            if out.len() >= cap {
                break;
            }
            let Some(entry) = snapshot.entry(id) else {
                continue;
            };
            if let Some(folder) = folder {
                let belongs = if direct_children {
                    entry.parent == folder
                } else {
                    snapshot.is_descendant_of(id, folder)
                };
                if !belongs {
                    continue;
                }
            }
            if !show_hidden && snapshot.is_hidden(id) {
                continue;
            }
            // A folder literally named `backup.png` is not a `.png` result.
            if has_ext_filter && entry.is_dir() {
                continue;
            }
            if !date_matches(opts.date, entry.mtime, cutoff) {
                continue;
            }
            let name = snapshot.name(entry);
            if !class_matches(opts.class, &name, entry.is_dir())
                || !name_passes(&name, &parsed.globs, &parsed.exts)
            {
                continue;
            }
            out.push((0, id));
        }
    };
    // Files first, so a capped Score over a big subtree surfaces the files the
    // user is looking for instead of 5 000 folders.
    let push = |out: &mut Vec<(u32, u32)>| match opts.scope {
        Scope::Files => push_range(snapshot.folder_count(), snapshot.len(), out),
        Scope::Folders => push_range(0, snapshot.folder_count(), out),
        Scope::All => {
            push_range(snapshot.folder_count(), snapshot.len(), out);
            if out.len() < cap {
                push_range(0, snapshot.folder_count(), out);
            }
        }
    };
    push(&mut scored);
    if cancelled() {
        return Err(Error::Cancelled);
    }
    apply_sort(snapshot, &mut scored, opts, cancelled)?;
    if opts.limit > 0 && scored.len() > opts.limit {
        scored.truncate(opts.limit);
    }
    Ok(take_ids(scored))
}

/// A Query split into fuzzy words, globs, and extension filters.
///
/// `.png` and `png` are extension filters (OR between them), `*.rs` is a glob,
/// everything else is a fuzzy word. `something .png` keeps both parts.
pub(crate) struct Parsed<'a> {
    pub fuzzy: Vec<&'a str>,
    pub globs: Vec<globset::GlobMatcher>,
    pub exts: Vec<String>,
}

pub(crate) fn parse_query(query: &str) -> Result<Parsed<'_>> {
    let mut out = Parsed {
        fuzzy: Vec::new(),
        globs: Vec::new(),
        exts: Vec::new(),
    };
    for token in query.split_whitespace() {
        if let Some(ext) = ext_token(token) {
            out.exts.push(ext.to_ascii_lowercase());
        } else if token.contains(['*', '?']) {
            let glob = GlobBuilder::new(token)
                .case_insensitive(true)
                .literal_separator(false)
                .build()
                .map_err(|e| Error::Query(e.to_string()))?;
            out.globs.push(glob.compile_matcher());
        } else if bare_ext_token(token) {
            out.exts.push(token.to_ascii_lowercase());
        } else {
            out.fuzzy.push(token);
        }
    }
    Ok(out)
}

/// Glob and extension filters from [`parse_query`], applied to one filename.
pub(crate) fn name_passes(name: &str, globs: &[globset::GlobMatcher], exts: &[String]) -> bool {
    (globs.is_empty() || globs.iter().all(|g| g.is_match(name)))
        && (exts.is_empty() || name_has_ext(name, exts))
}

/// `.wav` / `.exe` — extension filter, not a fuzzy atom. Several of these are OR.
fn ext_token(token: &str) -> Option<&str> {
    let ext = token.strip_prefix('.')?;
    if (1..=10).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Some(ext)
    } else {
        None
    }
}

fn bare_ext_token(token: &str) -> bool {
    const COMMON: &[&str] = &[
        "7z", "aac", "ai", "aiff", "apk", "avi", "blend", "bmp", "c", "cpp", "cs", "css", "csv",
        "doc", "docx", "dylib", "epub", "exe", "fish", "flac", "gif", "go", "gz", "h", "hpp",
        "html", "ico", "java", "jpeg", "jpg", "js", "json", "jsonl", "jsx", "kt", "log", "lua",
        "m4a", "md", "mkv", "mov", "mp3", "mp4", "ogg", "opus", "pdf", "php", "png", "ppt", "pptx",
        "psd", "py", "rar", "rb", "rs", "scss", "sh", "so", "sql", "svg", "swift", "tar", "toml",
        "ts", "tsx", "txt", "wav", "webm", "webp", "woff", "woff2", "xls", "xlsx", "xml", "xz",
        "yaml", "yml", "zip", "zsh",
    ];
    COMMON.iter().any(|ext| token.eq_ignore_ascii_case(ext))
}

fn name_has_ext(name: &str, exts: &[String]) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !ext.is_empty() && exts.iter().any(|want| ext.eq_ignore_ascii_case(want))
}

fn take_ids(scored: Vec<(u32, u32)>) -> Ranked {
    Ranked {
        ids: scored.into_iter().map(|(_, id)| id).collect(),
        indices: Vec::new(),
    }
}

fn apply_sort(
    snapshot: &Snapshot,
    scored: &mut [Scored],
    opts: SearchOpts,
    cancelled: &(impl Fn() -> bool + Sync),
) -> Result<()> {
    // Every ordering ends in an id tiebreak so the result is a total order:
    // equal-scoring Hits must not reshuffle between keystrokes just because
    // rayon happened to finish them in a different order.
    let meta = |id: u32| snapshot.entry(id);
    let mut by = |cmp: &dyn Fn(&Scored, &Scored) -> std::cmp::Ordering| {
        scored.sort_unstable_by(|a, b| cmp(a, b).then_with(|| a.1.cmp(&b.1)));
    };
    match opts.sort {
        Sort::Score => by(&|a, b| b.0.cmp(&a.0)),
        Sort::Name => by(&|a, b| cmp_name(snapshot, a.1, b.1)),
        Sort::NameDesc => by(&|a, b| cmp_name(snapshot, b.1, a.1)),
        // Size and age come from the snapshot the Rebuild already recorded, so
        // these cover every Hit and cost nothing, instead of statting a capped
        // prefix of the Catalog and silently dropping the rest. A stale entry
        // self-corrects on the next subtree refresh, and the row factories
        // re-read the live value for display.
        Sort::Newest => by(&|a, b| cmp_key(b, a, meta, |e: &Entry| e.mtime)),
        Sort::Oldest => by(&|a, b| cmp_key(a, b, meta, |e: &Entry| e.mtime)),
        Sort::Largest => by(&|a, b| cmp_key(b, a, meta, |e: &Entry| e.size)),
        Sort::Smallest => by(&|a, b| cmp_key(a, b, meta, |e: &Entry| e.size)),
    }
    if cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Order two Hits by a snapshot value, pushing entries whose value is still
/// unknown (`0`) last so an unmeasured folder never outranks a real file.
fn cmp_key<T: Ord + Default>(
    a: &Scored,
    b: &Scored,
    entry: impl Fn(u32) -> Option<Entry>,
    value: impl Fn(&Entry) -> T,
) -> std::cmp::Ordering {
    let key = |id: u32| entry(id).map_or_else(T::default, |e| value(&e));
    let (a, b) = (key(a.1), key(b.1));
    match (a != T::default(), b != T::default()) {
        (true, true) => a.cmp(&b),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => std::cmp::Ordering::Equal,
    }
}

/// Case-insensitive name order, matching what a live folder listing does.
fn cmp_name(snapshot: &Snapshot, a: u32, b: u32) -> std::cmp::Ordering {
    let fold = |id: u32| match snapshot.entry(id) {
        Some(e) => snapshot.name(e).to_lowercase(),
        None => String::new(),
    };
    fold(a).cmp(&fold(b))
}

fn score_only(pattern: &Pattern, name: &str) -> Option<u32> {
    thread_local! {
        static TLS: RefCell<(Matcher, Vec<char>)> = RefCell::new((
            Matcher::new(Config::DEFAULT.match_paths()),
            Vec::with_capacity(64),
        ));
    }
    TLS.with(|tls| {
        let (matcher, buf) = &mut *tls.borrow_mut();
        buf.clear();
        let hay = Utf32Str::new(name, buf);
        pattern.score(hay, matcher)
    })
}

fn highlight(pattern: &Pattern, name: &str) -> Option<Vec<u32>> {
    thread_local! {
        static TLS: RefCell<(Matcher, Vec<char>, Vec<u32>)> = RefCell::new((
            Matcher::new(Config::DEFAULT.match_paths()),
            Vec::with_capacity(64),
            Vec::with_capacity(16),
        ));
    }
    TLS.with(|tls| {
        let (matcher, buf, idx) = &mut *tls.borrow_mut();
        buf.clear();
        idx.clear();
        let hay = Utf32Str::new(name, buf);
        let _ = pattern.indices(hay, matcher, idx)?;
        Some(idx.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_tokens_survive_spaces_and_dots() {
        let p = parse_query("something .png psd *.rs").unwrap();
        assert_eq!(p.fuzzy, vec!["something"]);
        assert_eq!(p.exts, vec!["png", "psd"]);
        assert_eq!(p.globs.len(), 1);
        assert!(name_passes("shot.PNG", &p.globs[..0], &p.exts));
        assert!(name_passes("art.psd", &[], &p.exts));
        assert!(!name_passes("art.jpg", &[], &p.exts));
        assert!(!name_passes("art.psd", &p.globs, &p.exts));
        assert!(parse_query("something.png").unwrap().exts.is_empty());
    }
}
