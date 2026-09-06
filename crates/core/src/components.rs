//! Shared shell components. Native adapters render data and send explicit actions.
//! This module owns behavior; the registry owns labels, IDs and command discovery.
use crate::Manager;
use crate::process::CommandOutputExt;
use serde_json::{Value, json};
use std::io::Read;
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};

pub const REGISTRY: &str = include_str!("components.json");

pub fn registry() -> &'static Value {
    static REGISTRY_VALUE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    REGISTRY_VALUE
        .get_or_init(|| serde_json::from_str(REGISTRY).expect("embedded component registry"))
}

pub fn title(id: &str) -> &str {
    registry()["components"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == id))
        .and_then(|item| item["title"].as_str())
        .unwrap_or(id)
}

pub fn dispatch(manager: &Manager, component: &str, request: &str) -> Result<String, String> {
    if request.len() > 4 * 1024 * 1024 {
        return Err("Component request is too large".into());
    }
    let request: Value = serde_json::from_str(request).map_err(|error| error.to_string())?;
    if !request.is_object() {
        return Err("Component request must be a JSON object".into());
    }
    let action = string(&request, "action");
    let result = match component {
        "shell" => registry().clone(),
        "projects" => {
            if !matches!(action, "" | "list" | "refresh") {
                return Err("Unknown project action".into());
            }
            if action == "refresh" {
                crate::projects::refresh_project_account();
            }
            let catalog = manager
                .catalog()
                .ok_or("Project discovery needs an index. Build the index first.")?;
            let projects = crate::projects::index_projects(catalog)?;
            let map = manager.storage();
            json!({"projects": projects.into_iter().map(|project| {
                crate::FolderSizes::global().request(&project.path);
                json!({
                "path":project.path,"repository":project.repository,"branch":project.branch,
                "rust":project.rust,"node":project.node,"git":project.git,"modified":project.modified,
                "bytes":crate::FolderSizes::global().get(&project.path).or_else(|| map.and_then(|map| map.find_indexed(&project.path)).map(|entry|entry.bytes)),
                "artifacts":project.artifacts.into_iter().map(|(path,bytes)| {
                    crate::FolderSizes::global().request(&path);
                    let bytes=crate::FolderSizes::global().get(&path).or(bytes).or_else(|| map.and_then(|map|map.find_indexed(&path)).map(|entry|entry.bytes));
                    json!({"path":path,"bytes":bytes})
                }).collect::<Vec<_>>()
            })}).collect::<Vec<_>>()})
        }
        "git" => git_component(&directory(manager, &request)?, &request)?,
        "tasks" => task_component(&directory(manager, &request)?, &request)?,
        "storage" => storage_component(manager, &directory(manager, &request)?)?,
        "batch" => batch_component(&request)?,
        #[cfg(feature = "archives")]
        "archives" => archive_component(&request)?,
        _ => return Err(format!("Unknown shell component: {component}")),
    };
    serde_json::to_string(&result).map_err(|error| error.to_string())
}

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}
fn directory(manager: &Manager, request: &Value) -> Result<PathBuf, String> {
    let path = request["path"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| manager.directory().map(Path::to_path_buf))
        .ok_or("Choose a folder first")?;
    if !path.is_dir() {
        return Err(format!("Not a folder: {}", path.display()));
    }
    Ok(path)
}

/// The same Git runner backs GTK's rich diff renderer and the other native variants.
pub fn git(directory: &Path, args: &[&str], path: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(["--no-pager", "--no-optional-locks", "--literal-pathspecs"])
        .arg("-C")
        .arg(directory)
        .args(args);
    if let Some(path) = path {
        command.arg("--").arg(path);
    }
    let output = command
        .bounded_output(Duration::from_secs(20))
        .map_err(|error| error.to_string())?;
    if output.stdout.starts_with(b"[Showing last 512 KiB]\n") {
        return Err("Git output exceeds 512 KiB. Select one file or a smaller scope.".into());
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "{}{}",
            text,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn git_component(directory: &Path, request: &Value) -> Result<Value, String> {
    let root = PathBuf::from(git(directory, &["rev-parse", "--show-toplevel"], None)?.trim());
    let file = request["file"]
        .as_str()
        .filter(|file| !file.is_empty())
        .map(PathBuf::from);
    if file.as_ref().is_some_and(|file| {
        file.components()
            .any(|part| !matches!(part, Component::Normal(_)))
    }) {
        return Err("Choose a repository-relative file".into());
    }
    let action = string(request, "action");
    let staged = request["staged"].as_bool().unwrap_or(false);
    if matches!(action, "stage" | "unstage") {
        let file = file.as_deref().ok_or("Choose a file to stage or unstage")?;
        if action == "stage" {
            git(&root, &["add"], Some(file))?;
        } else if git(&root, &["rev-parse", "--verify", "HEAD"], None).is_ok() {
            git(&root, &["restore", "--staged"], Some(file))?;
        } else {
            git(&root, &["rm", "--cached"], Some(file))?;
        }
        return Ok(json!({"text":if action=="stage" {"File staged"}else{"File unstaged"}}));
    }
    let status = git(
        &root,
        &["status", "--short", "--branch", "--untracked-files=normal"],
        None,
    )?;
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    if staged {
        args.push("--cached");
    }
    let mut names = vec!["diff", "--name-only", "-z"];
    if staged {
        names.push("--cached");
    }
    let mut files = git(&root, &names, None)?;
    if !staged {
        files.push_str(&git(
            &root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            None,
        )?);
    }
    let mut files: Vec<_> = files.split('\0').filter(|file| !file.is_empty()).collect();
    files.sort();
    files.dedup();
    let text = match action {
        "status" => status.clone(),
        "" | "diff" => {
            let mut patch = git(&root, &args, file.as_deref())?;
            if patch.is_empty() && !staged {
                if let Some(file) = file.as_deref().filter(|file| root.join(file).is_file()) {
                    if git(&root, &["ls-files", "--error-unmatch"], Some(file)).is_err() {
                        let mut bytes = Vec::new();
                        fs::File::open(root.join(file))
                            .map_err(|error| error.to_string())?
                            .take(512 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .map_err(|error| error.to_string())?;
                        if bytes.len() > 512 * 1024 {
                            return Err(
                                "Untracked file is larger than the 512 KiB diff limit".into()
                            );
                        }
                        if bytes.contains(&0) {
                            patch = "Binary untracked file".into();
                        } else {
                            let content = String::from_utf8_lossy(&bytes);
                            patch = format!(
                                "--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n{}",
                                file.display(),
                                content.lines().count(),
                                content
                                    .lines()
                                    .map(|line| format!("+{line}\n"))
                                    .collect::<String>()
                            );
                        }
                    }
                }
            }
            if patch.is_empty() {
                "No changes for this selection.".into()
            } else {
                patch
            }
        }
        _ => return Err("Unknown Git action".into()),
    };
    Ok(json!({"text":text,"status":status,"files":files,"root":root}))
}

pub fn task_commands(path: &Path) -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    let mut commands = vec![
        (
            "git-status",
            "Git changes",
            vec!["git", "--no-pager", "status", "--short", "--branch"],
        ),
        (
            "git-log",
            "Git history",
            vec!["git", "--no-pager", "log", "-20", "--oneline", "--decorate"],
        ),
        ("git-fetch", "Git fetch", vec!["git", "fetch", "--all"]),
    ];
    if path.join("Cargo.toml").is_file() {
        commands.extend([
            ("cargo-check", "Rust check", vec!["cargo", "check"]),
            ("cargo-build", "Rust build", vec!["cargo", "build"]),
            (
                "cargo-release",
                "Rust release",
                vec!["cargo", "build", "--release"],
            ),
        ]);
    }
    if path.join("package.json").is_file() {
        commands.extend([
            ("npm-install", "npm install", vec!["npm", "install"]),
            ("npm-build", "npm build", vec!["npm", "run", "build"]),
            ("bun-install", "Bun install", vec!["bun", "install"]),
            ("bun-build", "Bun build", vec!["bun", "run", "build"]),
        ]);
    }
    commands
}

pub fn run_task(path: &Path, id: &str) -> Result<String, String> {
    let (_, title, args) = task_commands(path)
        .into_iter()
        .find(|(command, _, _)| *command == id)
        .ok_or("Unknown project command")?;
    let executable = if cfg!(windows) && args[0] == "npm" {
        "npm.cmd"
    } else {
        args[0]
    };
    let output = Command::new(executable)
        .args(&args[1..])
        .current_dir(path)
        .bounded_output(Duration::from_secs(1800));
    for directory in [
        path.to_path_buf(),
        path.join("target"),
        path.join("node_modules"),
    ] {
        if directory.is_dir() {
            crate::FolderSizes::global().invalidate(&directory);
        }
    }
    let output = output.map_err(|error| error.to_string())?;
    let report = format!(
        "{title}\nIn {}\nExit: {}\n\n{}{}",
        path.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(report)
    } else {
        Err(report)
    }
}

fn task_component(path: &Path, request: &Value) -> Result<Value, String> {
    match string(request, "action") {
        "" | "list" => Ok(
            json!({"commands":task_commands(path).into_iter().map(|(id,title,_)|json!({"id":id,"title":title})).collect::<Vec<_>>()}),
        ),
        "run" => Ok(json!({"text":run_task(path,string(request,"command"))?})),
        _ => Err("Unknown task action".into()),
    }
}

/// Storage inspection includes generated and hidden entries even when search excludes them.
pub fn storage_children(path: &Path) -> Result<Vec<crate::StorageEntry>, String> {
    fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .map(|entry| {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            if metadata.is_dir() {
                crate::FolderSizes::global().request(&entry.path());
            }
            Ok(crate::StorageEntry {
                id: u32::MAX,
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                bytes: if metadata.is_dir() {
                    crate::FolderSizes::global().get(&entry.path()).unwrap_or(0)
                } else {
                    metadata.len()
                },
                entries: 1,
            })
        })
        .collect()
}

fn storage_component(manager: &Manager, path: &Path) -> Result<Value, String> {
    let (free, total) = capacity(path)?;
    let mut entries = storage_children(path)?;
    for entry in &mut entries {
        if entry.is_dir {
            if let Some(indexed) = manager
                .storage()
                .and_then(|map| map.find_indexed(&entry.path))
            {
                entry.bytes = crate::FolderSizes::global()
                    .get(&entry.path)
                    .unwrap_or(indexed.bytes);
            }
        }
    }
    entries.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let remaining = entries
        .iter()
        .skip(256)
        .fold(0u64, |sum, entry| sum.saturating_add(entry.bytes));
    entries.truncate(256);
    let entries:Vec<_>=entries.into_iter().map(|entry|json!({"name":entry.name,"path":entry.path,"bytes":entry.bytes,"is_dir":entry.is_dir})).collect();
    Ok(json!({"entries":entries,"free":free,"total":total,"path":path,"remaining":remaining}))
}

/// Available and total filesystem bytes for any path on that filesystem.
pub fn capacity(path: &Path) -> Result<(u64, u64), String> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(path).map_err(|error| error.to_string())?;
        Ok((
            stat.f_bavail.saturating_mul(stat.f_frsize),
            stat.f_blocks.saturating_mul(stat.f_frsize),
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut available, mut total, mut free) = (0, 0, 0);
        // SAFETY: path is terminated UTF-16; all output pointers are live u64 values.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut available,
                &mut total,
                &mut free,
            )
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok((available, total))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err("Capacity is unavailable on this platform".into())
    }
}

pub fn rename_pairs(
    paths: &[PathBuf],
    find: &str,
    replace: &str,
    prefix: &str,
    suffix: &str,
    start: usize,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let sources: HashSet<_> = paths.iter().collect();
    let mut destinations = HashSet::new();
    paths
        .iter()
        .enumerate()
        .map(|(index, from)| {
            if from
                .ancestors()
                .skip(1)
                .any(|parent| sources.contains(&parent.to_path_buf()))
            {
                return Err("Select either a folder or its contents, not both".into());
            }
            let old = from
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Filename is not valid UTF-8")?;
            let name = format!(
                "{}{}{}",
                prefix.replace("{n}", &(index.saturating_add(start)).to_string()),
                if find.is_empty() {
                    old.into()
                } else {
                    old.replace(find, replace)
                },
                suffix
            );
            if name.is_empty()
                || matches!(name.as_str(), "." | "..")
                || name.contains(['/', '\\', '\0'])
            {
                return Err(format!("Invalid filename: {name:?}"));
            }
            let to = from.with_file_name(name);
            if !destinations.insert(to.clone()) {
                return Err(format!("Duplicate destination: {}", to.display()));
            }
            if from != &to && fs::symlink_metadata(&to).is_ok() {
                return Err(format!("Destination exists: {}", to.display()));
            }
            Ok((from.clone(), to))
        })
        .collect()
}

fn batch_component(request: &Value) -> Result<Value, String> {
    let paths: Vec<PathBuf> = request["paths"]
        .as_array()
        .ok_or("Select files first")?
        .iter()
        .map(|path| {
            let path = PathBuf::from(path.as_str().ok_or("Invalid selected path")?);
            if !path.is_absolute()
                || path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return Err("Selected paths must be absolute and normalized");
            }
            Ok(path)
        })
        .collect::<Result<_, _>>()?;
    if paths.is_empty() || paths.len() > 5000 {
        return Err("Select between one and 5,000 items".into());
    }
    if paths.iter().collect::<HashSet<_>>().len() != paths.len() {
        return Err("Selection contains duplicate paths".into());
    }
    let sources: HashSet<_> = paths.iter().map(PathBuf::as_path).collect();
    if paths.iter().any(|path| {
        path.ancestors()
            .skip(1)
            .any(|parent| sources.contains(parent))
    }) {
        return Err("Select either a folder or its contents, not both".into());
    }
    let action = string(request, "action");
    let pairs = if matches!(action, "rename_preview" | "rename") {
        rename_pairs(
            &paths,
            string(request, "find"),
            string(request, "replace"),
            string(request, "prefix"),
            string(request, "suffix"),
            request["start"].as_u64().unwrap_or(1) as usize,
        )?
    } else if matches!(action, "copy" | "move") {
        let destination = PathBuf::from(string(request, "destination"));
        let destination = destination
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !destination.is_dir() {
            return Err("Choose a destination folder".into());
        }
        let mut targets = HashSet::new();
        paths
            .iter()
            .map(|from| {
                let name = from
                    .file_name()
                    .ok_or("Cannot transfer a filesystem root")?;
                let to = destination.join(name);
                let source = from.canonicalize().map_err(|error| error.to_string())?;
                if to.starts_with(&source) {
                    return Err("Destination is inside the selection".into());
                }
                if fs::symlink_metadata(&to).is_ok() || !targets.insert(to.clone()) {
                    return Err(format!("Destination exists or repeats: {}", to.display()));
                }
                Ok((from.clone(), to))
            })
            .collect::<Result<Vec<_>, String>>()?
    } else {
        return Err("Unknown batch action".into());
    };
    for (from, _) in &pairs {
        fs::symlink_metadata(from).map_err(|error| format!("{}: {error}", from.display()))?;
    }
    let items: Vec<_> = pairs
        .iter()
        .map(|(from, to)| json!({"from":from,"to":to}))
        .collect();
    if action == "rename_preview" {
        return Ok(json!({"items":items,"text":format!("{} items ready for review",items.len())}));
    }
    let mut done = 0;
    for (from, to) in pairs.iter().filter(|(from, to)| from != to) {
        let result = match action {
            "rename" => crate::rename(from, to),
            "copy" => crate::copy(from, to),
            _ => crate::move_path(from, to),
        };
        result.map_err(|error| {
            format!(
                "Completed {done}; stopped at {}: {error}. Check partial output before retrying.",
                from.display()
            )
        })?;
        done += 1;
    }
    Ok(json!({"items":items,"text":format!("Completed {done} items")}))
}

#[cfg(feature = "archives")]
fn archive_component(request: &Value) -> Result<Value, String> {
    let destination = PathBuf::from(string(request, "destination"));
    match string(request, "action") {
        "open" => {
            let path = crate::archive::unpack(Path::new(string(request, "path")))
                .map_err(|error| error.to_string())?;
            Ok(json!({"path":path,"text":"Archive opened"}))
        }
        "compress" => {
            let paths = request["paths"]
                .as_array()
                .ok_or("Select files first")?
                .iter()
                .map(|value| value.as_str().map(PathBuf::from).ok_or("Invalid path"))
                .collect::<Result<Vec<_>, _>>()?;
            crate::archive::compress(&paths, &destination).map_err(|error| error.to_string())?;
            Ok(json!({"path":destination,"text":"Archive created"}))
        }
        "extract" => {
            crate::archive::extract(Path::new(string(request, "path")), &destination)
                .map_err(|error| error.to_string())?;
            Ok(json!({"path":destination,"text":"Archive extracted"}))
        }
        "save" => {
            let workspace = crate::archive::workspace(Path::new(string(request, "path")))
                .ok_or("This is not an archive workspace")?;
            crate::archive::repack(&workspace).map_err(|error| error.to_string())?;
            Ok(json!({"path":workspace.source,"text":"Archive saved"}))
        }
        _ => Err("Unknown archive action".into()),
    }
}
