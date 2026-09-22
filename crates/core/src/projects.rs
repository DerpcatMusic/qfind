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
    /// Base branch this project tracks (origin/main style). GitButler target-ref analogue.
    #[serde(default)]
    pub target: String,
    /// Ahead/behind vs upstream or target. Files branch-pill analogue.
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// Working-tree health: dirty (staged+unstaged) and untracked counts.
    #[serde(default)]
    pub dirty: u32,
    #[serde(default)]
    pub untracked: u32,
    #[serde(default)]
    pub conflicted: u32,
    /// Short last-commit summary (`sha message`).
    #[serde(default)]
    pub last_commit: String,
    /// Linked worktree paths sharing this repository.
    #[serde(default)]
    pub worktrees: Vec<PathBuf>,
    /// package.json script names (capped) for web projects.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// Preferred web toolchain detected from lockfiles.
    #[serde(default)]
    pub web_tool: String,
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

/// Walk up to the enclosing repository root (Files `repo_root` analogue).
/// Handles linked worktrees where `.git` is a file, not a directory.
pub fn repo_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn run_git(path: &std::path::Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .bounded_output(Duration::from_secs(5))
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `git status --porcelain=v1 --branch` header line
/// (`## main...origin/main [ahead 2, behind 1]`) into
/// `(branch, upstream, ahead, behind)`. One subprocess replaces the old
/// symbolic-ref + rev-list round trips.
pub(crate) fn parse_branch_header(line: &str) -> (String, String, u32, u32) {
    let body = line.strip_prefix("## ").unwrap_or(line);
    if body == "HEAD (no branch)" || body.starts_with("HEAD ") {
        return ("Detached HEAD".into(), String::new(), 0, 0);
    }
    if let Some(body) = body.strip_prefix("No commits yet on ") {
        return (body.to_owned(), String::new(), 0, 0);
    }
    let (branch, rest) = body.split_once("...").unwrap_or((body, ""));
    let branch = branch.to_owned();
    if rest.is_empty() {
        return (branch, String::new(), 0, 0);
    }
    let (upstream, flags) = rest.split_once(' ').unwrap_or((rest, ""));
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let flags = flags.trim_matches(|c| c == '[' || c == ']');
    for part in flags.split(", ") {
        if let Some(count) = part.strip_prefix("ahead ") {
            ahead = count.parse().unwrap_or(0);
        } else if let Some(count) = part.strip_prefix("behind ") {
            behind = count.parse().unwrap_or(0);
        }
    }
    // NOTE: keep `(ahead, behind)` order — rev-list style is `(behind, ahead)`.
    (branch, upstream.to_owned(), ahead, behind)
}

fn web_tool_and_scripts(path: &std::path::Path) -> (String, Vec<String>) {
    let manifest = fs::read_to_string(path.join("package.json")).unwrap_or_default();
    if manifest.is_empty() {
        return (String::new(), Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(&manifest).unwrap_or_default();
    let scripts = value["scripts"]
        .as_object()
        .map(|map| map.keys().take(24).cloned().collect())
        .unwrap_or_default();
    let tool = if path.join("bun.lockb").exists() || path.join("bun.lock").exists() {
        "bun"
    } else if path.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if path.join("yarn.lock").exists() {
        "yarn"
    } else if path.join("deno.json").exists() || path.join("deno.jsonc").exists() {
        "deno"
    } else {
        "npm"
    };
    (tool.into(), scripts)
}

fn github_repo_name(path: &std::path::Path, owned: &HashSet<String>) -> String {
    // Keep GitHub matching when an account is known; still record the
    // `owner/name` slug for foreign repos so local work stays visible.
    let urls = run_git(path, &["config", "--get-regexp", "remote\\..*\\.url"]).unwrap_or_default();
    let mut foreign = String::new();
    for line in urls.lines() {
        let Some(url) = line.split_whitespace().nth(1) else {
            continue;
        };
        let Some((_, repo)) = url
            .split_once("github.com:")
            .or_else(|| url.split_once("github.com/"))
        else {
            continue;
        };
        let repo = repo.trim_end_matches('/').trim_end_matches(".git").to_owned();
        if owned.contains(&repo.to_lowercase()) {
            return repo;
        }
        if foreign.is_empty() {
            foreign = repo;
        }
    }
    foreign
}

fn describe_project(path: PathBuf, owned: &HashSet<String>) -> Project {
    let rust = path.join("Cargo.toml").is_file();
    let node = path.join("package.json").is_file();
    let modified = [
        "Cargo.toml",
        "package.json",
        "bun.lockb",
        "pnpm-lock.yaml",
        ".git",
        "HEAD",
    ]
    .into_iter()
    .filter_map(|name| {
        let candidate = if name == "HEAD" {
            path.join(".git").join("HEAD")
        } else {
            path.join(name)
        };
        fs::metadata(candidate)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_secs() as i64)
    })
    .max()
    .unwrap_or(0);
    // Build/dependency caches worth reviewing, including web outputs.
    let mut artifacts = Vec::new();
    for name in [
        "target",
        "node_modules",
        "dist",
        "build",
        ".next",
        "coverage",
        ".venv",
    ] {
        let candidate = path.join(name);
        let wanted = match name {
            "target" => rust,
            "node_modules" | "dist" | "build" | ".next" | "coverage" => node,
            _ => true,
        };
        if wanted && candidate.is_dir() {
            artifacts.push((candidate, None));
        }
    }
    let repository = github_repo_name(&path, owned);
    // One `status` call yields branch, upstream, ahead/behind and health.
    let status = run_git(
        &path,
        &["status", "--porcelain=v1", "--branch", "--untracked-files=normal"],
    )
    .unwrap_or_default();
    let mut header = "";
    let mut dirty = 0u32;
    let mut untracked = 0u32;
    let mut conflicted = 0u32;
    for line in status.lines() {
        if let Some(branch_line) = line.strip_prefix("## ") {
            header = branch_line;
            continue;
        }
        if line.len() < 2 {
            continue;
        }
        let (x, y) = (line.as_bytes()[0] as char, line.as_bytes()[1] as char);
        if matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')) {
            conflicted += 1;
            dirty += 1;
        } else if x == '?' && y == '?' {
            untracked += 1;
        } else if x != ' ' || y != ' ' {
            dirty += 1;
        }
    }
    let (mut branch, upstream, mut ahead, mut behind) =
        parse_branch_header(&format!("## {header}"));
    if branch.is_empty() {
        branch = "Detached HEAD".into();
    }
    // Base branch: prefer the upstream from the status header; only then
    // spend one call on origin/HEAD. No rev-parse verify loop.
    let target = if upstream.is_empty() {
        run_git(&path, &["symbolic-ref", "refs/remotes/origin/HEAD"])
            .and_then(|text| {
                text.trim()
                    .strip_prefix("refs/remotes/")
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    } else {
        upstream
    };
    if ahead == 0 && behind == 0 && !target.is_empty() && !header.contains("...") {
        // No upstream tracking this branch: one rev-list vs the base branch.
        if let Some(counts) = run_git(
            &path,
            &["rev-list", "--left-right", "--count", &format!("{target}...HEAD")],
        ) {
            let mut parts = counts.split_whitespace();
            // rev-list prints `behind ahead`.
            behind = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            ahead = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        }
    }
    let last_commit = run_git(&path, &["log", "-1", "--format=%h %s"])
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    let (web_tool, scripts) = if node {
        web_tool_and_scripts(&path)
    } else {
        (String::new(), Vec::new())
    };
    Project {
        path,
        rust,
        node,
        git: true,
        repository,
        branch,
        modified,
        artifacts,
        target,
        ahead,
        behind,
        dirty,
        untracked,
        conflicted,
        last_commit,
        worktrees: Vec::new(),
        scripts,
        web_tool,
    }
}

fn worktree_paths(path: &std::path::Path) -> Vec<PathBuf> {
    let output = run_git(path, &["worktree", "list", "--porcelain", "-z"]).unwrap_or_default();
    output
        .split('\0')
        .filter_map(|record| record.strip_prefix("worktree ").map(PathBuf::from))
        .filter(|path| path.is_dir())
        .collect()
}

pub fn index_projects(catalog: &Catalog) -> Result<Vec<Project>, String> {
    // Offline-first dashboard: GitHub matching enriches local Hits but never
    // hides them. Without `gh auth`, every local repository is still listed.
    let login = active_project_account().unwrap_or_default();
    let overridden = ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let cache_dir = dirs::cache_dir().map(|path| path.join("qfind/projects"));
    let repo_list_cache = cache_dir.clone().map(|dir| {
        if login.is_empty() {
            dir.join("local-repos.txt")
        } else {
            dir.join(format!("github.com-{login}.txt"))
        }
    });
    let force = REFRESH_PROJECT_ACCOUNT.swap(false, std::sync::atomic::Ordering::Relaxed);
    let mut owned: HashSet<String> = HashSet::new();
    if !login.is_empty() {
        let fresh = repo_list_cache
            .as_ref()
            .filter(|_| !force)
            .and_then(|path| fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.elapsed().ok())
            .filter(|age| *age < Duration::from_secs(600))
            .and_then(|_| repo_list_cache.as_ref().and_then(|path| fs::read_to_string(path).ok()));
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
            match output {
                Ok(output) if output.status.success() => {
                    if !overridden {
                        let active = Command::new("gh").args(["config", "get", "user", "--host", "github.com"]).bounded_output(Duration::from_secs(15))
                            .ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_lowercase());
                        if active.as_deref() != Some(&login) { return Err("GitHub account changed during discovery. Refresh Projects.".into()); }
                    }
                    let text = String::from_utf8_lossy(&output.stdout).into_owned();
                    if let Some(cache) = &repo_list_cache
                        && let Some(parent) = cache.parent()
                            && fs::create_dir_all(parent).is_ok()
                                && let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) {
                                    use std::io::Write;
                                    if file.write_all(text.as_bytes()).is_ok() { let _ = file.persist(cache); }
                                }
                    text
                }
                _ => repo_list_cache.as_ref().and_then(|path| fs::read_to_string(path).ok())
                    .unwrap_or_default(),
            }
        };
        owned = repositories.lines().map(str::to_lowercase).collect();
    }
    let workspace_cache = cache_dir.clone().map(|dir| {
        if login.is_empty() {
            dir.join("local-workspaces.json")
        } else {
            dir.join(format!("github.com-{login}.workspaces.json"))
        }
    });
    let snapshot_stamp = fs::metadata(catalog.path())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_nanos().to_string());
    if !force
        && let Some(path) = workspace_cache.as_ref().filter(|path| {
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(600))
        })
            && let Some(value) = fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            {
                // Only the snapshot path gates the cache: the stamp changes on every
                // subtree refresh, which used to force a full project re-scan.
                if value["snapshot"] == serde_json::json!(catalog.path())
                    && let Ok(mut projects) =
                        serde_json::from_value::<Vec<Project>>(value["projects"].clone())
                    {
                        projects.retain(|project| project.path.is_dir());
                        if !login.is_empty() {
                            // Owned Hits first; local-only Hits stay visible below them.
                            projects.sort_by(|a, b| {
                                owned
                                    .contains(&b.repository.to_lowercase())
                                    .cmp(&owned.contains(&a.repository.to_lowercase()))
                            });
                        }
                        return Ok(projects);
                    }
            }
    let mut roots = std::collections::BTreeSet::new();
    for id in 0..catalog.len() {
        let Some(hit) = catalog.hit(id) else {
            continue;
        };
        if !matches!(
            hit.name(),
            "Cargo.toml" | "package.json" | ".git" | ".gitignore" | "bun.lockb" | "pnpm-lock.yaml" | "yarn.lock" | "deno.json"
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
    // git spawns dominate load time, so describe roots in parallel.
    // Each root costs ~3 fast git calls (status, remote urls, log).
    let mut projects: Vec<_> = {
        use rayon::prelude::*;
        roots
            .into_par_iter()
            .map(|path| describe_project(path, &owned))
            .collect()
    };
    // One common-dir call per project, reused for worktree expansion and
    // sibling attachment (previously 3 calls per project).
    let commons: Vec<String> = {
        use rayon::prelude::*;
        projects
            .par_iter()
            .map(|project| {
                run_git(
                    &project.path,
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                )
                .unwrap_or_else(|| project.path.to_string_lossy().into_owned())
            })
            .collect()
    };
    // Expand linked worktrees once per repository (GitButler worktree analogue).
    let mut seen: HashSet<_> = projects
        .iter()
        .map(|project| project.path.clone())
        .collect();
    let mut common_dirs = HashSet::new();
    let mut extra = Vec::new();
    for (project, common) in projects.iter().zip(&commons) {
        if !common_dirs.insert(common.clone()) {
            continue;
        }
        for path in worktree_paths(&project.path) {
            if seen.insert(path.clone()) {
                extra.push(describe_project(path, &owned));
            }
        }
    }
    projects.extend(extra);
    // Attach sibling worktrees to each row so the dashboard can expand them.
    // Cached commons cover known rows; only new worktree rows pay for one
    // extra rev-parse each.
    let mut by_common: std::collections::HashMap<String, Vec<PathBuf>> = std::collections::HashMap::new();
    let mut all_commons: Vec<String> = Vec::with_capacity(projects.len());
    for (index, project) in projects.iter().enumerate() {
        let common = commons.get(index).cloned().unwrap_or_else(|| {
            run_git(&project.path, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
                .unwrap_or_else(|| project.path.to_string_lossy().into_owned())
        });
        by_common
            .entry(common.clone())
            .or_default()
            .push(project.path.clone());
        all_commons.push(common);
    }
    for (project, common) in projects.iter_mut().zip(&all_commons) {
        if let Some(siblings) = by_common.get(common) {
            project.worktrees = siblings
                .iter()
                .filter(|path| *path != &project.path)
                .cloned()
                .collect();
        }
    }
    if !login.is_empty() {
        projects.sort_by(|a, b| {
            owned
                .contains(&b.repository.to_lowercase())
                .cmp(&owned.contains(&a.repository.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
    } else {
        projects.sort_by(|a, b| a.path.cmp(&b.path));
    }
    if let Some(path) = workspace_cache
        && let Some(parent) = path.parent()
            && fs::create_dir_all(parent).is_ok()
                && let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) {
                    let value = serde_json::json!({"snapshot":catalog.path(),"stamp":snapshot_stamp,"projects":projects});
                    if serde_json::to_writer(&mut file, &value).is_ok() {
                        let _ = file.persist(path);
                    }
                }
    Ok(projects)
}
