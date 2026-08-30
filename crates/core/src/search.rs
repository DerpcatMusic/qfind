use std::cell::RefCell;

use globset::GlobBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::prefilter;
use crate::query::{MatchMode, Scope, SearchOpts, Sort, class_matches, date_cutoff, date_matches};
use crate::snapshot::Snapshot;

pub(crate) struct Ranked {
    pub ids: Vec<u32>,
    pub indices: Vec<Vec<u32>>,
}

pub(crate) fn search(snapshot: &Snapshot, query: &str, opts: SearchOpts) -> Result<Ranked> {
    let mut globs = Vec::new();
    let mut fuzzy_parts = Vec::new();
    let mut exts = Vec::new();
    for token in query.split_whitespace() {
        if let Some(ext) = ext_token(token) {
            exts.push(ext);
        } else if token.contains(['*', '?']) {
            let glob = GlobBuilder::new(token)
                .case_insensitive(true)
                .literal_separator(false)
                .build()
                .map_err(|e| Error::Query(e.to_string()))?;
            globs.push(glob.compile_matcher());
        } else {
            fuzzy_parts.push(token);
        }
    }
    if fuzzy_parts.is_empty() {
        return Ok(scan_filtered(snapshot, opts, |name| {
            (globs.is_empty() || globs.iter().all(|g| g.is_match(name)))
                && (exts.is_empty() || name_has_ext(name, &exts))
        }));
    }

    let pattern = compile_pattern(&fuzzy_parts.join(" "), opts.match_mode);
    let cutoff = date_cutoff(opts.date);
    let keep = |id: u32| -> Option<&str> {
        let entry = snapshot.entry(id)?;
        if !exts.is_empty() && entry.is_dir() {
            return None;
        }
        if !date_matches(opts.date, entry.mtime, cutoff) {
            return None;
        }
        let name = snapshot.name(entry);
        if !class_matches(opts.class, name, entry.is_dir()) {
            return None;
        }
        if !globs.is_empty() && !globs.iter().all(|g| g.is_match(name)) {
            return None;
        }
        if !exts.is_empty() && !name_has_ext(name, &exts) {
            return None;
        }
        Some(name)
    };

    let need = prefilter::needle_mask(&fuzzy_parts);
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
    let score_one = |id: u32| {
        let name = keep(id)?;
        score_only(&pattern, name).map(|score| (score, id))
    };
    let mut scored: Vec<(u32, u32)> = if cands.len() < 4096 {
        cands.into_iter().filter_map(score_one).collect()
    } else {
        cands.into_par_iter().filter_map(score_one).collect()
    };

    apply_sort(snapshot, &mut scored, opts);
    if opts.limit > 0 && scored.len() > opts.limit {
        scored.truncate(opts.limit);
    }

    if !opts.highlight {
        return Ok(take_ids(scored));
    }

    let mut ids = Vec::with_capacity(scored.len());
    let mut indices = Vec::with_capacity(scored.len());
    for (_, id) in scored {
        let idx = snapshot
            .entry(id)
            .and_then(|e| highlight(&pattern, snapshot.name(e)))
            .unwrap_or_default();
        indices.push(idx);
        ids.push(id);
    }
    Ok(Ranked { ids, indices })
}

/// Cap live `stat` so Newest/Oldest/size stay interactive on huge Hit lists.
const STAT_CAP: usize = 20_000;

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

fn scan_filtered(snapshot: &Snapshot, opts: SearchOpts, extra: impl Fn(&str) -> bool) -> Ranked {
    let cutoff = date_cutoff(opts.date);
    // Empty / glob-only + Score: files first so browse isn't 5k folders and zero files.
    // Stop at `cap` so we don't allocate the whole Catalog.
    let cap = match opts.sort {
        Sort::Score if opts.limit > 0 => opts.limit,
        Sort::Score => usize::MAX,
        _ => STAT_CAP.max(opts.limit),
    };
    let mut scored = Vec::new();
    if cap < usize::MAX {
        scored.reserve(cap);
    }
    let mut push_range = |start: u32, end: u32| {
        for id in start..end {
            if scored.len() >= cap {
                break;
            }
            let Some(entry) = snapshot.entry(id) else {
                continue;
            };
            if !date_matches(opts.date, entry.mtime, cutoff) {
                continue;
            }
            let name = snapshot.name(entry);
            if !class_matches(opts.class, name, entry.is_dir()) || !extra(name) {
                continue;
            }
            scored.push((0, id));
        }
    };
    match opts.scope {
        Scope::Files => push_range(snapshot.folder_count(), snapshot.len()),
        Scope::Folders => push_range(0, snapshot.folder_count()),
        Scope::All => {
            push_range(snapshot.folder_count(), snapshot.len());
            push_range(0, snapshot.folder_count());
        }
    }
    apply_sort(snapshot, &mut scored, opts);
    if opts.limit > 0 && scored.len() > opts.limit {
        scored.truncate(opts.limit);
    }
    take_ids(scored)
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

fn name_has_ext(name: &str, exts: &[&str]) -> bool {
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

fn apply_sort(snapshot: &Snapshot, scored: &mut Vec<(u32, u32)>, opts: SearchOpts) {
    match opts.sort {
        // Stable on ties so empty Query can keep files-first insertion order.
        Sort::Score => scored.sort_by(|a, b| b.0.cmp(&a.0)),
        Sort::Name => {
            scored.sort_unstable_by(|a, b| cmp_name(snapshot, a.1, b.1).then(a.1.cmp(&b.1)))
        }
        Sort::NameDesc => {
            scored.sort_unstable_by(|a, b| cmp_name(snapshot, b.1, a.1).then(a.1.cmp(&b.1)))
        }
        Sort::Newest | Sort::Oldest | Sort::Largest | Sort::Smallest => {
            if scored.len() > STAT_CAP {
                scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                scored.truncate(STAT_CAP);
            }
            let mut meta: Vec<(u32, u32, u64, i64)> = scored
                .par_iter()
                .map(|&(score, id)| {
                    let (size, mtime) = live_meta(&snapshot.path(id));
                    (score, id, size, mtime)
                })
                .collect();
            match opts.sort {
                Sort::Newest => {
                    meta.sort_unstable_by(|a, b| b.3.cmp(&a.3).then(a.1.cmp(&b.1)));
                }
                Sort::Oldest => {
                    meta.sort_unstable_by(|a, b| a.3.cmp(&b.3).then(a.1.cmp(&b.1)));
                }
                Sort::Largest => {
                    meta.sort_unstable_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
                }
                Sort::Smallest => {
                    meta.sort_unstable_by(|a, b| a.2.cmp(&b.2).then(a.1.cmp(&b.1)));
                }
                _ => {}
            }
            *scored = meta.into_iter().map(|(s, id, _, _)| (s, id)).collect();
        }
    }
}

fn cmp_name(snapshot: &Snapshot, a: u32, b: u32) -> std::cmp::Ordering {
    let na = snapshot.entry(a).map(|e| snapshot.name(e)).unwrap_or("");
    let nb = snapshot.entry(b).map(|e| snapshot.name(e)).unwrap_or("");
    na.cmp(nb)
}

fn live_meta(path: &std::path::Path) -> (u64, i64) {
    match rustix::fs::stat(path) {
        Ok(st) => (st.st_size.max(0) as u64, st.st_mtime as i64),
        Err(_) => (0, 0),
    }
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
