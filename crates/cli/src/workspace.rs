use anyhow::{Context, Result, anyhow};
use clap::{Subcommand, ValueEnum};
use qfind_core::{Catalog, Manager, components, default_snapshot_path};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// Discover your repositories using the signed-in gh account and indexed worktrees
    Projects {
        #[arg(long)]
        refresh: bool,
        #[arg(long)]
        snapshot: Option<PathBuf>,
    },
    /// Repository status, diffs, and staging (paths are repository-relative)
    Git {
        #[arg(value_enum)]
        action: GitAction,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        staged: bool,
        #[arg(long)]
        json: bool,
    },
    /// List project commands, or run a command ID from that list
    Tasks {
        command: Option<String>,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Indexed directory weights, disk capacity, and free space
    Storage {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Show all indexed storage roots
        #[arg(long)]
        global: bool,
        #[arg(long)]
        snapshot: Option<PathBuf>,
    },
    /// Discover and invoke the same component protocol used by native GUI adapters
    Component {
        #[arg(default_value = "shell")]
        name: String,
        /// JSON request; use - to read from stdin
        #[arg(default_value = "{}")]
        request: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        snapshot: Option<PathBuf>,
    },
    /// Show shared preferences or their file location
    Config {
        #[arg(long, conflicts_with = "edit")]
        path: bool,
        /// Open shared preferences with the configured editor or desktop handler
        #[arg(long)]
        edit: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum GitAction {
    Status,
    Diff,
    Stage,
    Unstage,
}

fn manager(path: PathBuf, snapshot: Option<PathBuf>, required: bool) -> Result<Manager> {
    let path = path
        .canonicalize()
        .with_context(|| format!("open {}", path.display()))?;
    let required = required || snapshot.is_some();
    let snapshot = snapshot.unwrap_or_else(default_snapshot_path);
    match Catalog::open(&snapshot) {
        Ok(catalog) => Ok(Manager::new(catalog, Some(path))),
        Err(_) if !required => Ok(Manager::live(Some(path))),
        Err(error) => Err(error)
            .with_context(|| format!("open {} (run `qfind index` first)", snapshot.display())),
    }
}

fn dispatch(manager: &Manager, name: &str, request: Value, raw: bool) -> Result<()> {
    let response = components::dispatch(manager, name, &request.to_string())
        .map_err(|error| anyhow!(error))?;
    let value: Value = serde_json::from_str(&response)?;
    let mut out = io::stdout().lock();
    if !raw && let Some(text) = value["text"].as_str() {
        writeln!(out, "{text}")?;
    } else {
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
    }
    Ok(())
}

pub fn run(command: WorkspaceCommand) -> Result<()> {
    match command {
        WorkspaceCommand::Projects { refresh, snapshot } => dispatch(
            &manager(std::env::current_dir()?, snapshot, true)?,
            "projects",
            json!({"action": if refresh {"refresh"} else {"list"}}),
            true,
        ),
        WorkspaceCommand::Git {
            action,
            path,
            file,
            staged,
            json: raw,
        } => {
            let action = match action {
                GitAction::Status => "status",
                GitAction::Diff => "diff",
                GitAction::Stage => "stage",
                GitAction::Unstage => "unstage",
            };
            dispatch(
                &Manager::live(Some(path)),
                "git",
                json!({"action":action,"file":file,"staged":staged}),
                raw,
            )
        }
        WorkspaceCommand::Tasks {
            command,
            path,
            json: raw,
        } => dispatch(
            &Manager::live(Some(path)),
            "tasks",
            json!({"action": if command.is_some() {"run"} else {"list"}, "command":command}),
            raw,
        ),
        WorkspaceCommand::Storage {
            path,
            snapshot,
            global,
        } => {
            let manager = manager(path, snapshot, global)?;
            if global {
                let roots = manager.chart(true, usize::MAX)?;
                let entries: Vec<_> = roots
                    .into_iter()
                    .map(|row| {
                        let capacity = components::capacity(&row.path).ok();
                        json!({"path":row.path,"bytes":row.bytes,"entries":row.entries,"free":capacity.map(|(free,_)|free),"total":capacity.map(|(_,total)|total)})
                    })
                    .collect();
                writeln!(
                    io::stdout().lock(),
                    "{}",
                    serde_json::to_string_pretty(&json!({"roots":entries}))?
                )?;
                Ok(())
            } else {
                dispatch(&manager, "storage", json!({"action":"map"}), true)
            }
        }
        WorkspaceCommand::Component {
            name,
            mut request,
            path,
            snapshot,
        } => {
            if request == "-" {
                request.clear();
                io::stdin()
                    .take(4 * 1024 * 1024 + 1)
                    .read_to_string(&mut request)?;
            }
            anyhow::ensure!(
                request.len() <= 4 * 1024 * 1024,
                "Component request is too large"
            );
            let manager = if matches!(name.as_str(), "projects" | "storage") {
                manager(path, snapshot, name == "projects")?
            } else {
                Manager::live(Some(path))
            };
            dispatch(
                &manager,
                &name,
                serde_json::from_str(&request).context("parse component request JSON")?,
                true,
            )
        }
        WorkspaceCommand::Config { path, edit } => {
            if edit {
                let path = qfind_core::Config::path();
                if !path.exists() {
                    qfind_core::Config::load().save()?;
                }
                return crate::files::run(crate::files::FileCommand::Open { path });
            }
            let mut out = io::stdout().lock();
            if path {
                writeln!(out, "{}", qfind_core::Config::path().display())?;
            } else {
                write!(out, "{}", qfind_core::Config::load().to_toml())?;
            }
            Ok(())
        }
    }
}
