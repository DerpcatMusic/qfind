use crate::files::file_uri;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;

/// Places, bookmarks, and executable file-manager actions.
#[derive(Subcommand, Debug)]
pub enum PlaceCommand {
    /// List standard folders, pins, bookmarks, and mounted devices.
    List {
        /// Emit one JSON object per place.
        #[arg(long)]
        json: bool,
    },
    /// Add a directory to Qfind's pins.
    Pin {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Remove a directory from Qfind's pins.
    Unpin {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List or run executable Qfind and Nautilus actions.
    #[command(subcommand)]
    Actions(ActionCommand),
}

#[derive(Subcommand, Debug)]
pub enum ActionCommand {
    /// List actions with the IDs accepted by `actions run`.
    List {
        /// Emit one JSON object per action.
        #[arg(long)]
        json: bool,
    },
    /// Run an action by list ID or explicit executable path.
    Run {
        #[arg(value_name = "ID_OR_PATH")]
        action: String,
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
    },
}

pub fn run(command: PlaceCommand) -> Result<()> {
    match command {
        PlaceCommand::List { json } => list_places(json),
        PlaceCommand::Pin { path, json } => set_pin(path, true, json),
        PlaceCommand::Unpin { path, json } => set_pin(path, false, json),
        PlaceCommand::Actions(command) => run_actions(command),
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                Some(std::ffi::OsString::from(format!(
                    "{}{}",
                    drive.to_string_lossy(),
                    path.to_string_lossy()
                )))
            })
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn bookmark_file() -> Option<PathBuf> {
    Some(qfind_core::Config::path().with_file_name("bookmarks"))
}

fn qfind_bookmarks() -> Vec<PathBuf> {
    bookmark_file()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| {
            text.lines()
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn bookmarks() -> Vec<PathBuf> {
    let mut paths = qfind_bookmarks();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")));
    if let Some(config_home) = config_home {
        for file in [
            config_home.join("gtk-3.0/bookmarks"),
            config_home.join("gtk-4.0/bookmarks"),
        ] {
            if let Ok(text) = fs::read_to_string(file) {
                paths.extend(text.lines().filter_map(|line| {
                    let uri = line.split_whitespace().next()?;
                    uri_to_path(uri).filter(|path| path.is_dir())
                }));
            }
        }
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")));
    if let Some(data_home) = data_home
        && let Ok(text) = fs::read_to_string(data_home.join("user-places.xbel"))
    {
        paths.extend(text.split("href=\"").skip(1).filter_map(|tail| {
            let uri = tail.split('"').next()?;
            uri_to_path(uri).filter(|path| path.is_dir())
        }));
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

fn set_pin(path: PathBuf, active: bool, json_output: bool) -> Result<()> {
    let path = if active {
        path.canonicalize()
            .with_context(|| format!("resolve {}", path.display()))?
    } else if path.exists() {
        path.canonicalize()
            .with_context(|| format!("resolve {}", path.display()))?
    } else {
        path
    };
    if active && !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    let file = bookmark_file().context("HOME is not set")?;
    let mut saved = qfind_bookmarks();
    saved.retain(|candidate| candidate != &path);
    if active {
        saved.push(path.clone());
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).context("create Qfind bookmark directory")?;
    }
    fs::write(
        &file,
        saved
            .iter()
            .map(|saved| saved.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .with_context(|| format!("write {}", file.display()))?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "action": if active { "pinned" } else { "unpinned" },
                "path": path,
            }))?
        );
    } else {
        println!(
            "{} {}",
            if active { "Pinned" } else { "Unpinned" },
            path.display()
        );
    }
    Ok(())
}

fn list_places(json_output: bool) -> Result<()> {
    let mut places = Vec::new();
    let mut shown = HashSet::new();
    if let Some(home) = home_dir() {
        for (name, path) in [
            ("Home", home.clone()),
            ("Desktop", home.join("Desktop")),
            ("Documents", home.join("Documents")),
            ("Downloads", home.join("Downloads")),
            ("Music", home.join("Music")),
            ("Pictures", home.join("Pictures")),
            ("Videos", home.join("Videos")),
        ] {
            if path.is_dir() && shown.insert(path.clone()) {
                places.push(("standard", name.to_owned(), path));
            }
        }
    }
    for path in bookmarks() {
        if shown.insert(path.clone()) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Folder")
                .to_owned();
            places.push(("pinned", name, path));
        }
    }
    for path in mounts() {
        if shown.insert(path.clone()) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("Root")
                .to_owned();
            places.push(("device", name, path));
        }
    }
    for (kind, name, path) in places {
        if json_output {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "kind": kind,
                    "name": name,
                    "path": path,
                }))?
            );
        } else {
            println!("{kind}\t{name}\t{}", path.display());
        }
    }
    Ok(())
}

fn run_actions(command: ActionCommand) -> Result<()> {
    match command {
        ActionCommand::List { json } => list_actions(json),
        ActionCommand::Run { action, paths } => run_action(&action, paths),
    }
}

struct ExternalAction {
    source: &'static str,
    path: PathBuf,
}

fn actions() -> Vec<ExternalAction> {
    let Some(data_home) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (source, root) in [
        ("qfind", data_home.join("qfind/actions")),
        ("nautilus", data_home.join("nautilus/scripts")),
    ] {
        let mut paths = Vec::new();
        action_paths(&root, &mut paths);
        out.extend(
            paths
                .into_iter()
                .map(|path| ExternalAction { source, path }),
        );
    }
    out
}

fn action_paths(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read_dir) = fs::read_dir(root) else {
        return;
    };
    let mut entries = read_dir
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() && !path.is_symlink() {
            action_paths(&path, out);
        } else if path.is_file() && is_executable(&path) {
            out.push(path);
        }
    }
}

fn list_actions(json_output: bool) -> Result<()> {
    for (id, action) in actions().into_iter().enumerate() {
        let label = action_label(&action.path);
        if json_output {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "id": id,
                    "label": label,
                    "source": action.source,
                    "path": action.path,
                }))?
            );
        } else {
            println!(
                "{id}\t{label}\t{}\t{}",
                action.source,
                action.path.display()
            );
        }
    }
    Ok(())
}

fn run_action(spec: &str, paths: Vec<PathBuf>) -> Result<()> {
    let command = if let Ok(id) = spec.parse::<usize>() {
        actions()
            .into_iter()
            .nth(id)
            .map(|action| action.path)
            .with_context(|| format!("unknown action ID {id}"))?
    } else {
        PathBuf::from(spec)
    };
    if !command.is_file() || !is_executable(&command) {
        bail!("action is not an executable file: {}", command.display());
    }
    let paths = paths
        .into_iter()
        .map(|path| {
            fs::symlink_metadata(&path)
                .with_context(|| format!("read selected path {}", path.display()))?;
            std::path::absolute(&path)
                .with_context(|| format!("resolve selected path {}", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let selected = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let current = paths
        .first()
        .and_then(|path| path.parent())
        .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
    let selected_paths = selected.join("\n");
    let selected_uris = paths
        .iter()
        .map(|path| format!("{}\n", file_uri(path)))
        .collect::<String>();
    let status = Command::new(&command)
        .args(&paths)
        .env("QFIND_SELECTED_PATHS", &selected_paths)
        .env("QFIND_CURRENT_DIRECTORY", &current)
        .env(
            "NAUTILUS_SCRIPT_SELECTED_FILE_PATHS",
            format!("{selected_paths}\n"),
        )
        .env("NAUTILUS_SCRIPT_SELECTED_URIS", selected_uris)
        .env("NAUTILUS_SCRIPT_CURRENT_URI", file_uri(Path::new(&current)))
        .status()
        .with_context(|| format!("run action {}", command.display()))?;
    anyhow::ensure!(
        status.success(),
        "action {} exited with {status}",
        command.display()
    );
    Ok(())
}

fn action_label(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(['_', '-'], " ")
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.extension()
            .is_some_and(|ext| matches!(ext.to_str(), Some("exe" | "bat" | "cmd")))
    }
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = if let Some(path) = rest.strip_prefix('/') {
        format!("/{path}")
    } else {
        let (host, path) = rest.split_once('/')?;
        if !host.is_empty() && host != "localhost" {
            return None;
        }
        format!("/{path}")
    };
    let bytes = percent_decode(path.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        let mut path = String::from_utf8(bytes).ok()?;
        if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') {
            path.remove(0);
        }
        Some(PathBuf::from(path))
    }
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let hi = input.get(i + 1).and_then(|byte| hex(*byte))?;
            let lo = input.get(i + 2).and_then(|byte| hex(*byte))?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    Some(out)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn mounts() -> Vec<PathBuf> {
    qfind_core::discover_mounts()
}
