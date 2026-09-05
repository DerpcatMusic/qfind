//! Account-scoped project discovery shared by every native frontend.
use crate::Catalog;
use crate::process::CommandOutputExt;
use std::{collections::HashSet, fs, path::PathBuf, process::Command, time::Duration};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub path: PathBuf,
    pub rust: bool,
    pub node: bool,
    pub git: bool,
    pub repository: String,
    pub branch: String,
    pub modified: i64,
    pub artifacts: Vec<(PathBuf, Option<u64>)>,
}

static REFRESH_PROJECT_ACCOUNT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn refresh_project_account() {
    REFRESH_PROJECT_ACCOUNT.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn active_project_account() -> Result<String, String> {
    // Resolve the active account locally; environment tokens override gh's saved account.
    let overridden = ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let identity = if overridden {
        Command::new("gh")
            .args(["api", "--hostname", "github.com", "user", "--jq", ".login"])
            .bounded_output(Duration::from_secs(15))
    } else {
        Command::new("gh")
            .args(["config", "get", "user", "--host", "github.com"])
            .bounded_output(Duration::from_secs(15))
    }
    .map_err(|error| {
        format!("Could not read your GitHub account: {error}. Connect with gh auth login.")
    })?;
    let login = String::from_utf8_lossy(&identity.stdout)
        .trim()
        .to_lowercase();
    if !identity.status.success()
        || login.is_empty()
        || !login
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(
            "Connect your GitHub account with gh auth login, then refresh Projects.".into(),
        );
    }
    Ok(login)
}

pub fn index_projects(catalog: &Catalog) -> Result<Vec<Project>, String> {
    let login = active_project_account()?;
    let overridden = ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let cache = dirs::cache_dir().map(|path| {
        path.join("qfind/projects")
            .join(format!("github.com-{login}.txt"))
    });
    let force = REFRESH_PROJECT_ACCOUNT.swap(false, std::sync::atomic::Ordering::Relaxed);
    let fresh = cache
        .as_ref()
        .filter(|path| {
            !force
                && fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.elapsed().ok())
                    .is_some_and(|age| age < Duration::from_secs(600))
        })
        .and_then(|path| fs::read_to_string(path).ok());
    let repositories = if let Some(fresh) = fresh {
        fresh
    } else {
        let output = Command::new("gh")
            .args([
                "api",
                "--hostname",
                "github.com",
                "user/repos?per_page=100&affiliation=owner,collaborator,organization_member",
                "--paginate",
                "--jq",
                ".[] | select(.permissions.push == true) | .full_name",
            ])
            .bounded_output(Duration::from_secs(15));
        let repositories = match output {
            Ok(output) if output.status.success() => {
                if !overridden {
                    let active = Command::new("gh").args(["config", "get", "user", "--host", "github.com"]).bounded_output(Duration::from_secs(15))
                        .ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_lowercase());
                    if active.as_deref() != Some(&login) { return Err("GitHub account changed during discovery. Refresh Projects.".into()); }
                }
                let text = String::from_utf8_lossy(&output.stdout).into_owned();
                if let Some(cache) = &cache {
                    if let Some(parent) = cache.parent() {
                        if fs::create_dir_all(parent).is_ok() {
                            if let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) {
                                use std::io::Write;
                                if file.write_all(text.as_bytes()).is_ok() { let _ = file.persist(cache); }
                            }
                        }
                    }
                }
                text
            }
            _ => cache.as_ref().and_then(|path| fs::read_to_string(path).ok())
                .ok_or_else(|| format!("Could not load {login}'s GitHub repositories and no offline index exists. Check gh auth status and your connection, then refresh."))?,
        };
        repositories
    };
    let owned: HashSet<String> = repositories.lines().map(str::to_lowercase).collect();
    let workspace_cache = cache
        .as_ref()
        .map(|path| path.with_extension("workspaces.json"));
    let snapshot_stamp = fs::metadata(catalog.path())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_nanos().to_string());
    if !force {
        if let Some(path) = workspace_cache.as_ref().filter(|path| {
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(600))
        }) {
            if let Some(value) = fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            {
                if value["snapshot"] == serde_json::json!(catalog.path())
                    && value["stamp"] == serde_json::json!(snapshot_stamp)
                {
                    if let Ok(mut projects) =
                        serde_json::from_value::<Vec<Project>>(value["projects"].clone())
                    {
                        projects.retain(|project| {
                            project.path.is_dir()
                                && owned.contains(&project.repository.to_lowercase())
                        });
                        return Ok(projects);
                    }
                }
            }
        }
    }
    let mut roots = std::collections::BTreeSet::new();
    for id in 0..catalog.len() {
        let Some(hit) = catalog.hit(id) else {
            continue;
        };
        if !matches!(
            hit.name(),
            "Cargo.toml" | "package.json" | ".git" | ".gitignore"
        ) {
            continue;
        }
        let path = hit.path();
        let Some(parent) = path.parent() else {
            continue;
        };
        // Installed tool trees and dependencies are not user workspaces.
        if parent.components().any(|part| {
            part.as_os_str().to_str().is_some_and(|name| {
                matches!(
                    name,
                    "target"
                        | "node_modules"
                        | "vendor"
                        | "site-packages"
                        | ".cargo"
                        | ".rustup"
                        | ".npm"
                        | ".bun"
                        | ".cache"
                )
            })
        }) {
            continue;
        }
        if parent.join(".git").exists() {
            roots.insert(parent.to_path_buf());
        }
    }
    let describe = |path: PathBuf| {
        let rust = path.join("Cargo.toml").is_file();
        let node = path.join("package.json").is_file();
        let modified = ["Cargo.toml", "package.json", ".git"]
            .into_iter()
            .filter_map(|name| {
                fs::metadata(path.join(name))
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|time| time.as_secs() as i64)
            })
            .max()
            .unwrap_or(0);
        let artifacts = [("target", rust), ("node_modules", node)]
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(name, _)| path.join(name))
            .filter(|path| path.is_dir())
            .map(|path| (path, None))
            .collect();
        // Git resolves worktrees, includes and SSH/HTTPS remotes without assuming .git is a directory.
        let repository = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["config", "--get-regexp", "remote\\..*\\.url"])
            .bounded_output(Duration::from_secs(15))
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default()
            .lines()
            .find_map(|line| {
                let url = line.split_whitespace().nth(1)?;
                let (_, repo) = url
                    .split_once("github.com:")
                    .or_else(|| url.split_once("github.com/"))?;
                let repo = repo.trim_end_matches('/').trim_end_matches(".git");
                owned
                    .contains(&repo.to_lowercase())
                    .then(|| repo.to_owned())
            })
            .unwrap_or_default();
        let branch = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["symbolic-ref", "--short", "HEAD"])
            .bounded_output(Duration::from_secs(15))
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_else(|| "Detached HEAD".into());
        Project {
            path,
            rust,
            node,
            git: true,
            repository,
            branch,
            modified,
            artifacts,
        }
    };
    let mut projects: Vec<_> = roots
        .into_iter()
        .map(&describe)
        .filter(|project| !project.repository.is_empty())
        .collect();
    let mut seen: HashSet<_> = projects
        .iter()
        .map(|project| project.path.clone())
        .collect();
    let mut repositories = HashSet::new();
    let mut worktrees = Vec::new();
    for project in &projects {
        let common = Command::new("git")
            .arg("-C")
            .arg(&project.path)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .bounded_output(Duration::from_secs(15))
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_else(|| project.path.to_string_lossy().into_owned());
        if !repositories.insert(common) {
            continue;
        }
        if let Ok(output) = Command::new("git")
            .arg("-C")
            .arg(&project.path)
            .args(["worktree", "list", "--porcelain", "-z"])
            .bounded_output(Duration::from_secs(15))
        {
            for record in String::from_utf8_lossy(&output.stdout).split('\0') {
                if let Some(path) = record.strip_prefix("worktree ").map(PathBuf::from) {
                    if path.is_dir() && seen.insert(path.clone()) {
                        worktrees.push(path);
                    }
                }
            }
        }
    }
    projects.extend(
        worktrees
            .into_iter()
            .map(describe)
            .filter(|project| !project.repository.is_empty()),
    );
    if let Some(path) = workspace_cache {
        if let Some(parent) = path.parent() {
            if let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) {
                let value = serde_json::json!({"snapshot":catalog.path(),"stamp":snapshot_stamp,"projects":projects});
                if serde_json::to_writer(&mut file, &value).is_ok() {
                    let _ = file.persist(path);
                }
            }
        }
    }
    Ok(projects)
}
