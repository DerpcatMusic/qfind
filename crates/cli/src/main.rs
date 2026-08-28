use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use qfind_core::{
    Catalog, Config, DateAge, FileClass, MatchMode, Rebuild, Scope, SearchOpts, Sort,
    default_snapshot_path,
};

#[derive(Parser)]
#[command(name = "qfind", about = "Search filenames in the Qfind Catalog")]
struct Cli {
    /// Print paths NUL-separated
    #[arg(short = '0', long = "nul", global = true)]
    nul: bool,

    /// Folders only
    #[arg(long)]
    folders: bool,

    /// Files only
    #[arg(long)]
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
    #[arg(trailing_var_arg = true)]
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
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::from(1)
        }
    }
}

fn json_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Index { roots, snapshot }) => {
            let snapshot = snapshot.unwrap_or_else(default_snapshot_path);
            let cfg = Config::load();
            let mut rebuild = Rebuild::new(&snapshot);
            if !roots.is_empty() {
                rebuild = rebuild.roots(roots);
            } else if !cfg.include.is_empty() {
                rebuild = rebuild.roots(cfg.include.clone());
            }
            for e in &cfg.exclude {
                rebuild = rebuild.exclude(e);
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
            if cli.query.is_empty() && !cli.json {
                if io::stdout().is_terminal() {
                    qfind_tui::run()?;
                    return Ok(ExitCode::SUCCESS);
                }
                let snapshot = default_snapshot_path();
                let catalog = Catalog::open(&snapshot).with_context(|| {
                    format!("open {} (run `qfind index` first)", snapshot.display())
                })?;
                println!(
                    "{}  {} folders, {} files",
                    catalog.path().display(),
                    catalog.folder_count(),
                    catalog.file_count()
                );
                return Ok(ExitCode::SUCCESS);
            }
            let snapshot = default_snapshot_path();
            let catalog = Catalog::open(&snapshot).with_context(|| {
                format!("open {} (run `qfind index` first)", snapshot.display())
            })?;
            let query = cli.query.join(" ");
            let opts = SearchOpts {
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
            let hits = catalog.search_with(&query, opts)?;
            let mut out = io::stdout().lock();
            for hit in hits.iter() {
                let path = hit.path();
                if cli.json {
                    writeln!(
                        out,
                        "{{\"name\":\"{}\",\"path\":\"{}\",\"dir\":{}}}",
                        json_esc(hit.name()),
                        json_esc(&path.to_string_lossy()),
                        hit.is_dir()
                    )?;
                } else if cli.nul {
                    out.write_all(path.as_os_str().as_encoded_bytes())?;
                    out.write_all(&[0])?;
                } else {
                    writeln!(out, "{}", path.display())?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
