mod files;
#[cfg(target_os = "linux")]
#[path = "os_linux.rs"]
mod os;
#[cfg(target_os = "windows")]
#[path = "os_windows.rs"]
mod os;
#[cfg(target_os = "macos")]
#[path = "os_macos.rs"]
mod os;
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
#[path = "os_other.rs"]
mod os;
mod places;
mod workspace;

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use qfind_core::{
    Catalog, Config, DateAge, FileClass, IgnoreMatcher, MatchMode, Scope, SearchOpts, Sort,
    default_snapshot_path,
};

#[derive(Parser)]
#[command(
    name = "qfind",
    version,
    about = "Megaman: files, storage, projects, and indexed global search"
)]
struct Cli {
    /// Print paths NUL-separated
    #[arg(short = '0', long = "nul", global = true)]
    nul: bool,

    /// Use a particular search catalog
    #[arg(long)]
    snapshot: Option<PathBuf>,

    /// Search recursively inside this indexed directory
    #[arg(long = "in")]
    directory: Option<PathBuf>,

    /// Override the shared hidden-file preference
    #[arg(long, action = clap::ArgAction::Set)]
    hidden: Option<bool>,

    /// Folders only
    #[arg(long, conflicts_with = "files")]
    folders: bool,

    /// Files only
    #[arg(long, conflicts_with = "folders")]
    files: bool,

    /// Filter by FileClass
    #[arg(long, value_enum)]
    class: Option<ClassArg>,

    /// Order Hits
    #[arg(long, value_enum, default_value_t = SortArg::Score)]
    sort: SortArg,

    /// Keep Hits whose mtime is in this window (no-op when mtime is 0)
    #[arg(long, value_enum)]
    date: Option<DateArg>,

    /// Cap Hits (0 = no cap)
    #[arg(long, default_value_t = 0)]
    limit: usize,

    /// How loose the Query is
    #[arg(long = "match", value_enum, default_value_t = MatchArg::Fuzzy)]
    match_mode: MatchArg,

    /// One JSON object per Hit (Vicinae / Nautilus / scripts)
    #[arg(long)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,

    /// Query tokens (AND). Globs allowed: *.wav
    #[arg(num_args = 0..)]
    query: Vec<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ClassArg {
    Image,
    Audio,
    Video,
    Document,
    Archive,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortArg {
    Score,
    Name,
    NameDesc,
    Newest,
    Oldest,
    Largest,
    Smallest,
}

#[derive(Clone, Copy, ValueEnum)]
enum MatchArg {
    Fuzzy,
    Substring,
    Exact,
}

#[derive(Clone, Copy, ValueEnum)]
enum DateArg {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Subcommand)]
enum Command {
    /// Open the interactive terminal manager
    Tui,
    /// Bookmarks, mounted devices, and file-manager scripts
    #[command(subcommand)]
    Places(places::PlaceCommand),
    /// File operations, batch actions, and archives
    #[command(subcommand)]
    Files(files::FileCommand),
    /// Shared GUI workspaces: projects, Git, tasks, and storage
    #[command(flatten)]
    Workspace(workspace::WorkspaceCommand),
    /// Rebuild the Catalog from local Mounts
    Index {
        /// Mount to include (repeatable). Default: discover local disks
        #[arg(long = "root")]
        roots: Vec<PathBuf>,
        #[arg(long)]
        snapshot: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    os::init();
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Tui) => {
            let name = if cfg!(windows) {
                "qfind-tui.exe"
            } else {
                "qfind-tui"
            };
            let sibling = std::env::current_exe()?.with_file_name(name);
            let status = std::process::Command::new(if sibling.is_file() {
                sibling
            } else {
                PathBuf::from(name)
            })
            .status()
            .context("launch qfind-tui")?;
            Ok(ExitCode::from(
                status.code().unwrap_or(1).clamp(0, 255) as u8
            ))
        }
        Some(Command::Places(command)) => {
            places::run(command)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Files(command)) => {
            files::run(command)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Workspace(command)) => {
            workspace::run(command)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Index { roots, snapshot }) => {
            let snapshot = snapshot.unwrap_or_else(default_snapshot_path);
            let cfg = Config::load();
            let mut rebuild = cfg.rebuild_to(&snapshot);
            if !roots.is_empty() {
                rebuild = rebuild.roots(roots);
            }
            let catalog = Catalog::rebuild(rebuild)
                .with_context(|| format!("rebuild {}", snapshot.display()))?;
            eprintln!(
                "catalog {}  ({} folders, {} files)",
                catalog.path().display(),
                catalog.folder_count(),
                catalog.file_count()
            );
            Ok(ExitCode::SUCCESS)
        }
        None => {
            if cli.query.is_empty() && !cli.json && cli.directory.is_none() {
                let snapshot = cli.snapshot.clone().unwrap_or_else(default_snapshot_path);
                let catalog = Catalog::open(&snapshot).with_context(|| {
                    format!("open {} (run `qfind index` first)", snapshot.display())
                })?;
                println!(
                    "{}  {} folders, {} files",
                    catalog.path().display(),
                    catalog.folder_count(),
                    catalog.file_count()
                );
                if io::stdout().is_terminal() {
                    eprintln!("run `qfind-tui` for the interactive UI");
                }
                return Ok(ExitCode::SUCCESS);
            }
            let snapshot = cli.snapshot.clone().unwrap_or_else(default_snapshot_path);
            let catalog = Catalog::open(&snapshot).with_context(|| {
                format!("open {} (run `qfind index` first)", snapshot.display())
            })?;
            let query = cli.query.join(" ");
            let mut opts = SearchOpts {
                scope: if cli.folders {
                    Scope::Folders
                } else if cli.files {
                    Scope::Files
                } else {
                    Scope::All
                },
                class: match cli.class {
                    Some(ClassArg::Image) => FileClass::Image,
                    Some(ClassArg::Audio) => FileClass::Audio,
                    Some(ClassArg::Video) => FileClass::Video,
                    Some(ClassArg::Document) => FileClass::Document,
                    Some(ClassArg::Archive) => FileClass::Archive,
                    None => FileClass::All,
                },
                sort: match cli.sort {
                    SortArg::Score => Sort::Score,
                    SortArg::Name => Sort::Name,
                    SortArg::NameDesc => Sort::NameDesc,
                    SortArg::Newest => Sort::Newest,
                    SortArg::Oldest => Sort::Oldest,
                    SortArg::Largest => Sort::Largest,
                    SortArg::Smallest => Sort::Smallest,
                },
                date: match cli.date {
                    Some(DateArg::Day) => DateAge::Day,
                    Some(DateArg::Week) => DateAge::Week,
                    Some(DateArg::Month) => DateAge::Month,
                    Some(DateArg::Year) => DateAge::Year,
                    None => DateAge::Any,
                },
                limit: cli.limit,
                highlight: false,
                match_mode: match cli.match_mode {
                    MatchArg::Fuzzy => MatchMode::Fuzzy,
                    MatchArg::Substring => MatchMode::Substring,
                    MatchArg::Exact => MatchMode::Exact,
                },
            };
            let cfg = Config::load();
            let mut ignores = IgnoreMatcher::new(cfg.respect_gitignore, cfg.respect_ignore);
            let limit = opts.limit;
            if ignores.is_some() && limit > 0 {
                opts.limit = 0;
            }
            let show_hidden = cli.hidden.unwrap_or(cfg.show_hidden);
            let folder = cli
                .directory
                .as_ref()
                .map(|path| {
                    let path = path
                        .canonicalize()
                        .with_context(|| format!("open {}", path.display()))?;
                    catalog
                        .folder(&path)
                        .with_context(|| format!("directory not indexed: {}", path.display()))
                })
                .transpose()?;
            let hits = if let Some(folder) = &folder {
                folder.search_with_hidden_cancel(&query, opts, show_hidden, || false)?
            } else {
                catalog.search_with_hidden_cancel(&query, opts, show_hidden, || false)?
            };
            let mut out = io::stdout().lock();
            let mut emitted = 0usize;
            for hit in hits.iter() {
                let path = hit.path();
                if ignores
                    .as_mut()
                    .is_some_and(|matcher| matcher.is_ignored(&path, hit.is_dir()))
                {
                    continue;
                }
                if cli.json {
                    writeln!(
                        out,
                        "{}",
                        serde_json::json!({
                            "name":hit.name(), "path":path.to_string_lossy(), "dir":hit.is_dir(),
                            "bytes":hit.size(), "modified":hit.mtime()
                        })
                    )?;
                } else if cli.nul {
                    #[cfg(windows)]
                    out.write_all(path.to_string_lossy().as_bytes())?;
                    #[cfg(not(windows))]
                    out.write_all(path.as_os_str().as_encoded_bytes())?;
                    out.write_all(&[0])?;
                } else {
                    writeln!(out, "{}", path.display())?;
                }
                emitted += 1;
                if limit > 0 && emitted >= limit {
                    break;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
