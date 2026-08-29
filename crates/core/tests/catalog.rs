use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use qfind_core::{Catalog, FileClass, MatchMode, Rebuild, Scope, SearchOpts, Sort};

fn write_file(path: &Path, body: &str) {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).expect("mkdir");
    }
    let mut f = File::create(path).expect("create");
    f.write_all(body.as_bytes()).expect("write");
}

fn set_mtime(path: &Path, secs: i64) {
    let ts = rustix::fs::Timestamps {
        last_access: rustix::fs::Timespec {
            tv_sec: secs,
            tv_nsec: 0,
        },
        last_modification: rustix::fs::Timespec {
            tv_sec: secs,
            tv_nsec: 0,
        },
    };
    rustix::fs::utimensat(
        rustix::fs::CWD,
        path,
        &ts,
        rustix::fs::AtFlags::empty(),
    )
    .expect("utimensat");
}

fn names(catalog: &Catalog, query: &str) -> Vec<String> {
    catalog
        .search(query)
        .expect("search")
        .iter()
        .map(|h| h.name().to_string())
        .collect()
}

#[test]
fn rebuild_and_search_through_catalog_interface() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    write_file(&tree.join("hello.txt"), "hi");
    write_file(&tree.join("kick.wav"), "wav");
    write_file(&tree.join("sub").join("foo.txt"), "foo");
    write_file(
        &tree.join("node_modules").join("pkg").join("index.js"),
        "js",
    );

    let snapshot = tmp.path().join("catalog");
    let catalog =
        Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");

    assert!(snapshot.exists());
    assert!(catalog.file_count() >= 3);

    let hello = names(&catalog, "hello");
    assert!(hello.iter().any(|n| n == "hello.txt"), "{hello:?}");

    let wavs = names(&catalog, "*.wav");
    assert!(wavs.iter().any(|n| n == "kick.wav"), "{wavs:?}");
    let dot_wav = names(&catalog, ".wav");
    assert!(
        dot_wav.iter().any(|n| n == "kick.wav"),
        ".wav should be an extension filter: {dot_wav:?}"
    );
    assert!(
        dot_wav.iter().all(|n| n.ends_with(".wav")),
        ".wav must not rank other extensions first: {dot_wav:?}"
    );
    let hello_txt = names(&catalog, "hello .txt");
    assert!(hello_txt.iter().any(|n| n == "hello.txt"), "{hello_txt:?}");
    assert!(
        hello_txt.iter().all(|n| n.ends_with(".txt")),
        "hello .txt must keep .txt first: {hello_txt:?}"
    );

    let and = names(&catalog, "foo txt");
    assert!(and.iter().any(|n| n == "foo.txt"), "{and:?}");

    let junk = names(&catalog, "index.js");
    assert!(
        junk.iter().all(|n| n != "index.js"),
        "node_modules leaked: {junk:?}"
    );

    let fuzzy = names(&catalog, "hlo");
    assert!(
        fuzzy.iter().any(|n| n == "hello.txt"),
        "fuzzy hlo -> hello.txt: {fuzzy:?}"
    );
    let ranked = catalog.search("hlo").expect("fuzzy");
    let first = ranked.get(0).expect("hit");
    assert_eq!(first.name(), "hello.txt");
    assert!(!first.indices().is_empty());

    let tight = catalog
        .search_with(
            "hlo",
            SearchOpts {
                match_mode: MatchMode::Substring,
                ..SearchOpts::default()
            },
        )
        .expect("substring");
    assert!(
        tight.iter().all(|h| h.name() != "hello.txt"),
        "substring must not gap-match hlo → hello"
    );
    let exact = catalog
        .search_with(
            "hello.txt",
            SearchOpts {
                match_mode: MatchMode::Exact,
                ..SearchOpts::default()
            },
        )
        .expect("exact");
    assert!(exact.iter().any(|h| h.name() == "hello.txt"));
    let exact_miss = catalog
        .search_with(
            "hello",
            SearchOpts {
                match_mode: MatchMode::Exact,
                ..SearchOpts::default()
            },
        )
        .expect("exact miss");
    assert!(exact_miss.iter().all(|h| h.name() != "hello.txt"));
}

#[test]
fn empty_query_returns_every_hit() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    write_file(&tree.join("a.txt"), "a");
    let snapshot = tmp.path().join("catalog");
    let catalog =
        Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");
    let all = catalog.search("").expect("search");
    assert!(all.len() >= 2, "root folder + file");
    let first_file = catalog
        .search_with(
            "",
            SearchOpts {
                limit: 1,
                ..SearchOpts::default()
            },
        )
        .expect("browse")
        .get(0)
        .expect("hit");
    assert!(!first_file.is_dir(), "empty Query browse is files-first");

    let glob_first = catalog
        .search_with(
            "*.txt",
            SearchOpts {
                limit: 1,
                ..SearchOpts::default()
            },
        )
        .expect("glob")
        .get(0)
        .expect("hit");
    assert!(!glob_first.is_dir(), "glob-only Score browse is files-first");
    assert!(glob_first.name().ends_with(".txt"));
}

#[test]
fn open_roundtrip_preserves_hits() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    write_file(&tree.join("round.txt"), "r");
    let snapshot = tmp.path().join("catalog");
    Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");
    let catalog = Catalog::open(&snapshot).expect("open");
    let hit = catalog
        .search("round")
        .expect("search")
        .iter()
        .find(|h| h.name() == "round.txt")
        .expect("hit");
    assert!(hit.path().ends_with("round.txt"));
    assert!(!hit.is_dir());
}

#[test]
fn extra_exclude_hides_named_folder() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    write_file(&tree.join("keep.txt"), "k");
    write_file(&tree.join("secret").join("nope.txt"), "n");
    let snapshot = tmp.path().join("catalog");
    let catalog = Catalog::rebuild(
        Rebuild::new(&snapshot)
            .roots([tree.as_path()])
            .exclude("secret"),
    )
    .expect("rebuild");
    let hits = names(&catalog, "nope");
    assert!(hits.is_empty(), "{hits:?}");
    assert!(names(&catalog, "keep").iter().any(|n| n == "keep.txt"));
}

#[test]
fn search_with_scope_class_sort_and_limit() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    write_file(&tree.join("photo.jpg"), "j");
    write_file(&tree.join("notes.txt"), "n");
    write_file(&tree.join("sub").join("clip.wav"), "w");
    let snapshot = tmp.path().join("catalog");
    let catalog =
        Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");
    catalog.warm();

    let folders = catalog
        .search_with(
            "",
            SearchOpts {
                scope: Scope::Folders,
                limit: 50,
                ..SearchOpts::default()
            },
        )
        .expect("folders");
    assert!(folders.iter().all(|h| h.is_dir()));
    assert!(folders.len() >= 1);

    let images = catalog
        .search_with(
            "photo",
            SearchOpts {
                class: FileClass::Image,
                ..SearchOpts::default()
            },
        )
        .expect("images");
    assert!(images.iter().any(|h| h.name() == "photo.jpg"));
    assert!(images.iter().all(|h| h.name().ends_with(".jpg")));

    let limited = catalog
        .search_with(
            "t",
            SearchOpts {
                limit: 1,
                sort: Sort::Name,
                ..SearchOpts::default()
            },
        )
        .expect("limit");
    assert_eq!(limited.len(), 1);

    let named = catalog
        .search_with(
            "t",
            SearchOpts {
                sort: Sort::Name,
                highlight: false,
                ..SearchOpts::default()
            },
        )
        .expect("name sort");
    let names: Vec<_> = named.iter().map(|h| h.name().to_string()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn newest_sort_uses_live_mtime() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    let old = tree.join("old.txt");
    let new = tree.join("new.txt");
    write_file(&old, "o");
    write_file(&new, "n");
    // Do not sleep. The walk root is stored as its full path (…/tree), so
    // fuzzy "txt" also matches that directory. Creating new.txt bumps the
    // dir mtime; Newest live-stats seconds (st_mtime), and id 0 wins ties.
    set_mtime(&old, 1_000_000);
    set_mtime(&new, 2_000_000);
    let snapshot = tmp.path().join("catalog");
    let catalog =
        Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");
    let hits = catalog
        .search_with(
            ".txt",
            SearchOpts {
                sort: Sort::Newest,
                highlight: false,
                ..SearchOpts::default()
            },
        )
        .expect("newest");
    let names: Vec<_> = hits.iter().map(|h| h.name().to_string()).collect();
    assert_eq!(names.first().map(String::as_str), Some("new.txt"), "{names:?}");

    let oldest = catalog
        .search_with(
            ".txt",
            SearchOpts {
                sort: Sort::Oldest,
                highlight: false,
                ..SearchOpts::default()
            },
        )
        .expect("oldest");
    let names: Vec<_> = oldest.iter().map(|h| h.name().to_string()).collect();
    assert_eq!(names.first().map(String::as_str), Some("old.txt"), "{names:?}");
}

#[test]
fn nested_path_and_quoted_name_roundtrip() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("tree");
    write_file(&tree.join("a").join("b").join("c").join("quote\"x.txt"), "q");
    let snapshot = tmp.path().join("catalog");
    let catalog =
        Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");
    let hit = catalog
        .search("quote")
        .expect("search")
        .iter()
        .find(|h| h.name().contains("quote"))
        .expect("hit");
    let path = hit.path();
    assert!(path.ends_with("quote\"x.txt"), "{}", path.display());
    assert!(path.components().count() >= 4);
}

#[test]
fn empty_catalog_search_is_empty() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let tree = tmp.path().join("empty");
    std::fs::create_dir_all(&tree).expect("mkdir");
    let snapshot = tmp.path().join("catalog");
    let catalog =
        Catalog::rebuild(Rebuild::new(&snapshot).roots([tree.as_path()])).expect("rebuild");
    let hits = catalog.search("zzzzmissing").expect("search");
    assert!(hits.is_empty());
}
