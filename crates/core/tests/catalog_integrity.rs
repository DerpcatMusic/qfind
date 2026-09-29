//! Integrity guarantees the Catalog has to hold, each of which was a real bug.
//! Anything here that can point at the wrong file is a data-loss bug, so these
//! are deliberately end-to-end through a real snapshot on disk.
use qfind_core::{Catalog, Config, SearchOpts, Sort};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qfind-integrity-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn catalog(root: &Path, include: Vec<PathBuf>) -> Catalog {
    let rebuild = Config {
        include,
        ..Default::default()
    }
    .rebuild_to(root.join("snap.bin"));
    Catalog::rebuild(rebuild).expect("rebuild")
}

fn hit<'a>(cat: &'a Catalog, needle: &str) -> qfind_core::Hit<'a> {
    cat.search_with(needle, SearchOpts::default())
        .unwrap()
        .ids()
        .iter()
        .map(|&i| cat.hit(i).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no Hit for {needle}"))
}

fn set_mtime(path: &Path, secs_ago: u64) {
    use std::time::{Duration, SystemTime};
    let when = SystemTime::now() - Duration::from_secs(secs_ago * 86_400);
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(when))
        .unwrap();
}

/// A non-UTF-8 filename used to be interned with an *empty* name, so
/// `Hit::path()` collapsed to the parent directory. The resulting blank row,
/// when trashed or deleted, wiped the whole folder.
#[cfg(unix)]
#[test]
fn non_utf8_name_keeps_its_own_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = scratch("utf8");
    let dir = root.join("mix");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("keep.txt"), b"k").unwrap();
    let odd = dir.join(OsStr::from_bytes(b"caf\xe9.txt"));
    fs::write(&odd, b"x").unwrap();

    let cat = catalog(&root, vec![dir.clone()]);
    let found = cat
        .search_with("", SearchOpts::default())
        .unwrap()
        .ids()
        .iter()
        .map(|&i| cat.hit(i).unwrap())
        .find(|h| h.name().contains('\u{FFFD}'))
        .expect("the non-UTF-8 file must be listed");

    assert_eq!(found.path(), odd, "path must be the file, not its parent");
    assert!(found.path().is_file());
    assert_ne!(found.path(), dir, "must never alias the parent directory");
}

/// The walk handed `mtime = 0` to every entry, so the Modified column was
/// permanently empty and `DateAge` filtering matched everything.
#[test]
fn walk_records_mtime_so_newest_sort_is_real() {
    let root = scratch("mtime");
    let dir = root.join("m");
    fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.txt");
    let new = dir.join("new.txt");
    fs::write(&old, b"x").unwrap();
    fs::write(&new, b"x").unwrap();
    set_mtime(&old, 30);
    set_mtime(&new, 1);

    let cat = catalog(&root, vec![dir.clone()]);
    assert!(hit(&cat, "old.txt").mtime() > 0, "mtime must be recorded");
    assert!(hit(&cat, "new.txt").mtime() > hit(&cat, "old.txt").mtime());

    // Scope::Files so the answer is about file order; a folder's own mtime is
    // when its last entry changed, which is legitimately "nowest".
    let newest = cat
        .search_with(
            "",
            SearchOpts {
                sort: Sort::Newest,
                scope: qfind_core::Scope::Files,
                ..Default::default()
            },
        )
        .unwrap();
    let names: Vec<_> = newest
        .ids()
        .iter()
        .map(|&i| cat.hit(i).unwrap().name().into_owned())
        .collect();
    assert_eq!(names, vec!["new.txt".to_owned(), "old.txt".to_owned()]);
}

/// `DateAge` used to treat a missing mtime as "matches every window", so
/// `--date day` happily returned files from 1995.
#[test]
fn date_window_actually_filters_on_mtime() {
    use qfind_core::DateAge;
    let root = scratch("date");
    let dir = root.join("m");
    fs::create_dir_all(&dir).unwrap();
    let old = dir.join("ancient.txt");
    let new = dir.join("fresh.txt");
    fs::write(&old, b"x").unwrap();
    fs::write(&new, b"x").unwrap();
    set_mtime(&old, 900);
    set_mtime(&new, 1);

    let cat = catalog(&root, vec![dir.clone()]);
    let today = cat
        .search_with(
            "txt",
            SearchOpts {
                date: DateAge::Day,
                ..Default::default()
            },
        )
        .unwrap();
    let names: Vec<_> = today
        .ids()
        .iter()
        .map(|&i| cat.hit(i).unwrap().name().into_owned())
        .collect();
    assert_eq!(names, vec!["fresh.txt".to_owned()], "today: {names:?}");
}

/// A symlinked folder used to be interned as a *file* whose "size" was the
/// length of the link target string, so it was unopenable and weighed nothing.
#[cfg(unix)]
#[test]
fn symlinked_folder_is_a_folder_with_real_size() {
    let root = scratch("symlink");
    let real = root.join("real");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("payload.bin"), vec![0u8; 4096]).unwrap();
    let link = root.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let cat = catalog(&root, vec![root.clone()]);
    let found = cat
        .search_with("link", SearchOpts::default())
        .unwrap()
        .ids()
        .iter()
        .map(|&i| cat.hit(i).unwrap())
        .find(|h| h.name() == "link")
        .expect("symlinked folder must be indexed");
    assert!(found.is_dir(), "a symlink to a folder is a folder");
    assert_eq!(found.path(), link);
    // A linked tree is walked once, not twice: the inode guard stops the same
    // bytes being indexed under both paths and double-counted in the WeightMap.
    let copies = cat
        .search_with("payload", SearchOpts::default())
        .unwrap()
        .ids()
        .iter()
        .map(|&i| cat.hit(i).unwrap())
        .filter(|h| h.name() == "payload.bin")
        .count();
    assert_eq!(copies, 1, "the linked tree must not be indexed twice");
    let child = hit(&cat, "payload");
    assert_eq!(child.size(), 4096, "must report the target's bytes");
}

/// A link pointing back up the tree must not make the walk run forever.
#[cfg(unix)]
#[test]
fn symlink_loop_terminates() {
    let root = scratch("loop");
    let deep = root.join("a/b");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("leaf.txt"), b"x").unwrap();
    std::os::unix::fs::symlink(&root, deep.join("up")).unwrap();

    let cat = catalog(&root, vec![root.clone()]);
    assert!(!cat.is_empty(), "walk must terminate and find the tree");
}

/// `folder_id` was an exact `PathBuf` match, so a case-different path on a
/// case-insensitive filesystem reported "not indexed" for a folder that is.
#[test]
fn folder_id_accepts_an_equivalent_spelling() {
    let root = scratch("folderid");
    let dir = root.join("Proj");
    fs::create_dir_all(&dir).unwrap();
    let cat = catalog(&root, vec![root.clone()]);
    assert!(cat.folder(&dir).is_some(), "exact path");
    assert!(
        cat.folder(root.join("Proj").join("")).is_some(),
        "trailing separator"
    );
    assert!(cat.folder(root.join("./Proj")).is_some(), "cur component");
}

/// The config writer escaped quotes and commas, but the reader was
/// `v.split(',')` plus `trim_matches('"')`. A Mount or Exclude rule containing
/// either came back corrupted, silently dropping a whole Mount from the Catalog.
#[test]
fn config_roundtrip_preserves_commas_and_quotes() {
    use qfind_core::Config;

    let path = scratch("config").join("config.toml");
    let cfg = Config {
        include: vec![
            PathBuf::from("/mnt/My Drive, Backup"),
            PathBuf::from("/mnt/say \"hello\""),
        ],
        exclude: vec![r"a,b".into(), r"back\slash".into(), "with\"quote".into()],
        editor: r#"nvim -c "set number""#.into(),
        ..Config::default()
    };
    fs::write(&path, cfg.to_toml()).unwrap();

    let back = Config::load_from(&path);
    assert_eq!(back.include, cfg.include, "include round trip");
    assert_eq!(back.exclude, cfg.exclude, "exclude round trip");
    assert_eq!(back.editor, cfg.editor, "editor round trip");
}

/// A boolean has to read the same way whichever spelling a hand-edit used, and
/// in the same direction. `show_hidden` used "anything but false" while
/// `zebra` used "only exactly true", so `zebra = True` silently became false.
#[test]
fn config_bools_read_consistently() {
    use qfind_core::Config;

    let path = scratch("bools").join("config.toml");
    let read = |body: &str| {
        fs::write(&path, body).unwrap();
        Config::load_from(&path)
    };
    for (yes, no) in [("true", "false"), ("yes", "no"), ("on", "off"), ("1", "0")] {
        let cfg = read(&format!(
            "zebra = {yes}\nshow_hidden = {yes}\nweight_map = {yes}\n"
        ));
        assert!(cfg.zebra && cfg.show_hidden && cfg.weight_map, "{yes}");
        let cfg = read(&format!(
            "zebra = {no}\nshow_hidden = {no}\nweight_map = {no}\n"
        ));
        assert!(!cfg.zebra && !cfg.show_hidden && !cfg.weight_map, "{no}");
    }
}

/// `~` used to stay literal, so `include = ["~/projects"]` produced a Catalog
/// that was quietly missing that tree with no error at any point.
#[test]
fn config_expands_home_in_paths() {
    use qfind_core::Config;

    let path = scratch("home").join("config.toml");
    fs::write(&path, "include = [\"~/projects\"]\n").unwrap();
    let cfg = Config::load_from(&path);
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    assert_eq!(cfg.include, vec![home.join("projects")]);
}

/// `check_dest_free` then `fs::rename` is a TOCTOU: POSIX `rename` replaces the
/// destination, so anything that created it in between was silently destroyed.
/// The replacement has to be refused inside the kernel.
#[test]
fn rename_never_replaces_an_occupied_destination() {
    use qfind_core::Mutation;

    let root = scratch("rename");
    let from = root.join("from.txt");
    let to = root.join("to.txt");
    fs::write(&from, b"new").unwrap();
    fs::write(&to, b"precious").unwrap();

    assert!(
        matches!(
            qfind_core::rename(&from, &to),
            Err(qfind_core::Error::AlreadyExists(_))
        ),
        "an occupied destination must be refused"
    );
    assert_eq!(fs::read(&to).unwrap(), b"precious", "must not be clobbered");
    assert_eq!(fs::read(&from).unwrap(), b"new", "source must survive");

    // And the same for a move, which used the same check-then-rename pair.
    assert!(matches!(
        qfind_core::move_path(&from, &to),
        Err(qfind_core::Error::AlreadyExists(_))
    ));
    assert_eq!(fs::read(&to).unwrap(), b"precious");

    // Freeing the destination makes the same call succeed.
    fs::remove_file(&to).unwrap();
    assert_eq!(
        qfind_core::rename(&from, &to).unwrap(),
        Mutation::Renamed {
            from: from.clone(),
            to: to.clone()
        }
    );
    assert_eq!(fs::read(&to).unwrap(), b"new");
}

/// The trash sidecar has to follow the freedesktop spec or Nautilus cannot
/// parse it. It used to emit epoch seconds and an unencoded path.
#[test]
fn trashinfo_follows_the_freedesktop_spec() {
    let root = scratch("trashinfo");
    // `trash_root()` is `<data>/Trash/files`, with the sidecar in the sibling
    // `info/` directory.
    let can = root.join("Trash").join("files");
    // A name with `%`, `#`, and a space: all of them break a raw `Path=`.
    let original = root.join("odd #1 100%.txt");
    fs::write(&original, b"x").unwrap();
    let (staged, _) = qfind_core::trash_into(&can, &original).unwrap();

    let info = can.parent().unwrap().join("info").join(format!(
        "{}.trashinfo",
        staged.file_name().unwrap().to_string_lossy()
    ));
    let body = fs::read_to_string(&info).unwrap();
    let path_line = body.lines().find_map(|l| l.strip_prefix("Path=")).unwrap();
    let date_line = body
        .lines()
        .find_map(|l| l.strip_prefix("DeletionDate="))
        .unwrap();
    let decoded = percent_decode(path_line);
    assert_eq!(
        decoded,
        original.to_string_lossy(),
        "Path= must percent-decode back to the original"
    );
    assert!(
        !path_line.contains(' '),
        "a raw space breaks the key: {path_line:?}"
    );
    assert!(
        !path_line.contains('#'),
        "a raw # breaks the key: {path_line:?}"
    );
    // ISO 8601, not epoch seconds.
    assert_eq!(
        date_line.len(),
        19,
        "DeletionDate must be ISO 8601 local time, got {date_line:?}"
    );
    assert_eq!(&date_line[4..5], "-", "{date_line:?}");
    assert_eq!(&date_line[7..8], "-", "{date_line:?}");
    assert_eq!(&date_line[10..11], "T", "{date_line:?}");
    assert!(body.starts_with("[Trash Info]"), "{}", body);
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).expect("decoded path must be UTF-8")
}

/// A broken symlink in the trash reported "free" to `Path::exists`, so the
/// rename replaced it. Collisions are about the *name*, not the target.
#[cfg(unix)]
#[test]
fn trash_collision_check_sees_a_broken_symlink() {
    let root = scratch("trashlink");
    let can = root.join("Trash");
    fs::create_dir_all(&can).unwrap();
    std::os::unix::fs::symlink(root.join("gone"), can.join("thing")).unwrap();

    let original = root.join("thing");
    fs::write(&original, b"real").unwrap();
    let (staged, _) = qfind_core::trash_into(&can, &original).unwrap();
    assert_ne!(
        staged.file_name().unwrap(),
        "thing",
        "must not land on the existing link"
    );
    assert_eq!(
        fs::read_link(can.join("thing")).unwrap(),
        root.join("gone"),
        "the pre-existing link must be untouched"
    );
    assert_eq!(fs::read(&staged).unwrap(), b"real");
}
