use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use qfind_core::{DateAge, FileClass, MatchMode, Scope, SearchOpts, Sort};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FileSort {
    Name,
    NameDesc,
    Newest,
    Oldest,
    Largest,
    Smallest,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FileMatch {
    Fuzzy,
    Substring,
    Exact,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FileClassArg {
    Image,
    Audio,
    Video,
    Document,
    Archive,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Folder to inspect.
    #[arg(value_name = "PATH", default_value = ".")]
    path: PathBuf,
    /// Filename filter. Whitespace separates AND terms.
    #[arg(short, long, alias = "query", default_value = "")]
    filter: String,
    /// Matching mode for the filename filter.
    #[arg(long = "match", value_enum, default_value_t = FileMatch::Fuzzy)]
    match_mode: FileMatch,
    /// Restrict by file kind.
    #[arg(long, alias = "type", value_enum)]
    class: Option<FileClassArg>,
    /// Restrict files by extension; repeat or comma-separate values.
    #[arg(long, alias = "extension", value_delimiter = ',')]
    extensions: Vec<String>,
    /// Show files only.
    #[arg(long, conflicts_with = "folders")]
    files: bool,
    /// Show folders only.
    #[arg(long, conflicts_with = "files")]
    folders: bool,
    /// Restrict by modification age.
    #[arg(long, value_enum)]
    date: Option<FileDate>,
    /// Order entries.
    #[arg(long, value_enum, default_value_t = FileSort::Name)]
    sort: FileSort,
    /// Maximum number of entries; zero means unlimited.
    #[arg(long, default_value_t = 0)]
    limit: usize,
    /// Emit only paths, suitable for composing another files command.
    #[arg(long, conflicts_with = "json")]
    paths_only: bool,
    /// Read the top-level global NUL option.
    #[arg(from_global)]
    nul: bool,
    /// Emit one JSON object per entry.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FileDate {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Args, Debug)]
pub struct PathsArgs {
    #[arg(value_name = "PATH", required = true, num_args = 1..)]
    paths: Vec<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
pub enum FileCommand {
    /// List immediate children of a folder.
    #[command(alias = "browse")]
    List(ListArgs),
    /// Create an empty file, creating missing parents.
    #[command(name = "create-file", aliases = ["touch", "file"])]
    CreateFile {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Create a folder and missing parents.
    #[command(name = "create-dir", aliases = ["mkdir", "directory"])]
    CreateDir {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Copy one file or folder tree to a free destination path.
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Move one file or folder tree to a free destination path.
    Move {
        from: PathBuf,
        to: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Copy selected paths into an existing folder.
    #[command(name = "batch-copy")]
    BatchCopy {
        destination: PathBuf,
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Move selected paths into an existing folder.
    #[command(name = "batch-move")]
    BatchMove {
        destination: PathBuf,
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Rename one file or folder without replacing an existing path.
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Move paths to Qfind's recoverable trash.
    Trash(PathsArgs),
    /// Restore a trashed path to its original path.
    Restore {
        trashed: PathBuf,
        original: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Permanently delete paths. This cannot be undone.
    #[command(name = "permanent-delete", aliases = ["delete", "rm"])]
    PermanentDelete(PathsArgs),
    /// Preview or apply a batch rename using the same rules as the GUI.
    #[command(name = "batch-rename", alias = "rename-batch")]
    BatchRename {
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "")]
        find: String,
        #[arg(long, default_value = "")]
        replace: String,
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(long, default_value = "")]
        suffix: String,
        #[arg(long, default_value_t = 1)]
        start: usize,
        /// Apply the preview. Without this flag, no filesystem changes occur.
        #[arg(long, conflicts_with = "preview")]
        apply: bool,
        /// Explicitly request the default preview-only mode.
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        json: bool,
    },
    /// Open, create, extract, or save an archive workspace.
    #[command(subcommand)]
    Archive(ArchiveCommand),
    /// Open a path using Qfind's configured editor or desktop handler.
    Open { path: PathBuf },
    /// Show a path in the platform file manager.
    #[command(aliases = ["open-folder", "show-in-files"])]
    Reveal { path: PathBuf },
    /// Run an explicit program with a path appended as its final argument.
    #[command(name = "open-with")]
    OpenWith {
        path: PathBuf,
        program: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Open a platform previewer when one is installed.
    Preview { path: PathBuf },
    /// Print a path, suitable for piping to a clipboard tool.
    #[command(name = "copy-path")]
    CopyPath { path: PathBuf },
    /// Print only a path's filename, suitable for piping to a clipboard tool.
    #[command(name = "copy-name")]
    CopyName { path: PathBuf },
    /// Print a path as a file URI, suitable for piping to a clipboard tool.
    #[command(name = "copy-uri")]
    CopyUri { path: PathBuf },
    /// Print filesystem metadata for a path.
    Properties {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ArchiveCommand {
    /// Materialize an archive in Qfind's cache and print its workspace path.
    Open {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Create a new archive. The first positional argument is the destination.
    Compress {
        destination: PathBuf,
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Extract an archive into a newly created directory.
    Extract {
        source: PathBuf,
        destination: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Save changes made in an opened writable archive workspace.
    Save {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: FileCommand) -> Result<()> {
    match command {
        FileCommand::List(args) => list(args),
        FileCommand::CreateFile { path, json } => {
            qfind_core::create_file(&path).context("create file")?;
            emit(
                json,
                json!({"action":"created","path":path}),
                format!("Created {}", path.display()),
            )
        }
        FileCommand::CreateDir { path, json } => {
            qfind_core::create_dir(&path).context("create directory")?;
            emit(
                json,
                json!({"action":"created-dir","path":path}),
                format!("Created {}", path.display()),
            )
        }
        FileCommand::Copy { from, to, json } => {
            qfind_core::copy(&from, &to).context("copy")?;
            emit(
                json,
                json!({"action":"copied","from":from,"to":to}),
                format!("Copied {} → {}", from.display(), to.display()),
            )
        }
        FileCommand::Move { from, to, json } => {
            qfind_core::move_path(&from, &to).context("move")?;
            emit(
                json,
                json!({"action":"moved","from":from,"to":to}),
                format!("Moved {} → {}", from.display(), to.display()),
            )
        }
        FileCommand::BatchCopy {
            destination,
            paths,
            json,
        } => batch_transfer(paths, &destination, false, json),
        FileCommand::BatchMove {
            destination,
            paths,
            json,
        } => batch_transfer(paths, &destination, true, json),
        FileCommand::Rename { from, to, json } => {
            qfind_core::rename(&from, &to).context("rename")?;
            emit(
                json,
                json!({"action":"renamed","from":from,"to":to}),
                format!("Renamed {} → {}", from.display(), to.display()),
            )
        }
        FileCommand::Trash(args) => trash(args),
        FileCommand::Restore {
            trashed,
            original,
            json,
        } => {
            qfind_core::restore(&trashed, &original).context("restore")?;
            emit(
                json,
                json!({"action":"restored","from":trashed,"to":original}),
                format!("Restored {} → {}", trashed.display(), original.display()),
            )
        }
        FileCommand::PermanentDelete(args) => permanent_delete(args),
        FileCommand::BatchRename {
            paths,
            find,
            replace,
            prefix,
            suffix,
            start,
            apply,
            preview: _,
            json,
        } => {
            let paths = paths
                .iter()
                .map(|path| absolute_path(path))
                .collect::<Result<Vec<_>>>()?;
            batch(
                json!({"action":if apply {"rename"} else {"rename_preview"},
                "paths":paths,"find":find,"replace":replace,"prefix":prefix,"suffix":suffix,"start":start}),
                json,
            )
        }
        FileCommand::Archive(command) => archive(command),
        FileCommand::Open { path } => open(&path),
        FileCommand::Reveal { path } => reveal(&path),
        FileCommand::OpenWith {
            path,
            program,
            args,
        } => open_with(&path, &program, &args),
        FileCommand::Preview { path } => preview(&path),
        FileCommand::CopyPath { path } => {
            println!("{}", path.display());
            Ok(())
        }
        FileCommand::CopyName { path } => copy_name(&path),
        FileCommand::CopyUri { path } => {
            println!("{}", file_uri(&path));
            Ok(())
        }
        FileCommand::Properties { path, json } => properties(&path, json),
    }
}

fn list(args: ListArgs) -> Result<()> {
    let class = match args.class {
        Some(FileClassArg::Image) => FileClass::Image,
        Some(FileClassArg::Audio) => FileClass::Audio,
        Some(FileClassArg::Video) => FileClass::Video,
        Some(FileClassArg::Document) => FileClass::Document,
        Some(FileClassArg::Archive) => FileClass::Archive,
        None => FileClass::All,
    };
    let sort = match args.sort {
        FileSort::Name => Sort::Name,
        FileSort::NameDesc => Sort::NameDesc,
        FileSort::Newest => Sort::Newest,
        FileSort::Oldest => Sort::Oldest,
        FileSort::Largest => Sort::Largest,
        FileSort::Smallest => Sort::Smallest,
    };
    let match_mode = match args.match_mode {
        FileMatch::Fuzzy => MatchMode::Fuzzy,
        FileMatch::Substring => MatchMode::Substring,
        FileMatch::Exact => MatchMode::Exact,
    };
    let date = match args.date {
        Some(FileDate::Day) => DateAge::Day,
        Some(FileDate::Week) => DateAge::Week,
        Some(FileDate::Month) => DateAge::Month,
        Some(FileDate::Year) => DateAge::Year,
        None => DateAge::Any,
    };
    let directory = args
        .path
        .canonicalize()
        .with_context(|| format!("open {}", args.path.display()))?;
    let mut rows = qfind_core::live_children(
        &directory,
        &args.filter,
        SearchOpts {
            scope: if args.files {
                Scope::Files
            } else if args.folders {
                Scope::Folders
            } else {
                Scope::All
            },
            class,
            sort,
            date: DateAge::Any,
            // The core browse helper truncates unconditionally; apply the
            // CLI limit after date filtering so the result count stays correct.
            limit: usize::MAX,
            highlight: false,
            match_mode,
        },
        true,
        true,
    )
    .with_context(|| format!("list {}", args.path.display()))?;
    let sizes = qfind_core::FolderSizes::global();
    for row in rows.iter_mut().filter(|row| row.is_dir) {
        sizes.request(&row.path);
        row.size = sizes.get(&row.path).unwrap_or(0);
    }
    if matches!(sort, Sort::Largest | Sort::Smallest) {
        rows.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| {
                if sort == Sort::Largest {
                    b.size.cmp(&a.size)
                } else {
                    a.size.cmp(&b.size)
                }
            })
        });
    }
    if date != DateAge::Any {
        let cutoff = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(match date {
                DateAge::Day => 86_400,
                DateAge::Week => 604_800,
                DateAge::Month => 2_592_000,
                DateAge::Year => 31_536_000,
                DateAge::Any => 0,
            });
        rows.retain(|row| row.mtime == 0 || (row.mtime > 0 && row.mtime as u64 >= cutoff));
    }
    if !args.extensions.is_empty() {
        let extensions: Vec<_> = args
            .extensions
            .iter()
            .map(|extension| {
                extension
                    .trim()
                    .trim_start_matches('.')
                    .to_ascii_lowercase()
            })
            .filter(|extension| !extension.is_empty())
            .collect();
        rows.retain(|row| {
            !row.is_dir
                && row
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extensions
                            .iter()
                            .any(|wanted| wanted == &extension.to_ascii_lowercase())
                    })
        });
    }
    if args.limit > 0 {
        rows.truncate(args.limit);
    }
    for row in rows {
        let value = json!({
            "name": row.name,
            "path": row.path,
            "dir": row.is_dir,
            "size": if row.is_dir { sizes.get(&row.path) } else { Some(row.size) },
            "mtime": row.mtime,
        });
        if args.paths_only {
            if args.nul {
                write_path_nul(&row.path)?;
            } else {
                println!("{}", row.path.display());
            }
        } else if args.json {
            println!("{}", serde_json::to_string(&value)?);
        } else {
            println!(
                "{}\t{}",
                if row.is_dir { "d" } else { "f" },
                row.path.display()
            );
        }
    }
    Ok(())
}

fn write_path_nul(path: &Path) -> Result<()> {
    let mut out = io::stdout().lock();
    #[cfg(windows)]
    out.write_all(path.to_string_lossy().as_bytes())?;
    #[cfg(not(windows))]
    out.write_all(path.as_os_str().as_encoded_bytes())?;
    out.write_all(&[0])?;
    Ok(())
}

fn checked_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(
        !paths.is_empty() && paths.len() <= 5_000,
        "select between one and 5,000 paths"
    );
    let paths = paths
        .iter()
        .map(|path| absolute_path(path))
        .collect::<Result<Vec<_>>>()?;
    let selected: std::collections::HashSet<_> = paths.iter().collect();
    anyhow::ensure!(
        selected.len() == paths.len(),
        "selection contains duplicate paths"
    );
    for path in &paths {
        fs::symlink_metadata(path).with_context(|| format!("read {}", path.display()))?;
        anyhow::ensure!(
            !path
                .ancestors()
                .skip(1)
                .any(|parent| selected.contains(&parent.to_path_buf())),
            "select either a folder or its contents, not both"
        );
    }
    Ok(paths)
}

fn trash(args: PathsArgs) -> Result<()> {
    for path in checked_paths(&args.paths)? {
        let (trashed, _) =
            qfind_core::trash(&path).with_context(|| format!("trash {}", path.display()))?;
        emit(
            args.json,
            json!({"action":"trashed","items":[{"from":path,"to":trashed}]}),
            format!("Trashed {} → {}", path.display(), trashed.display()),
        )?;
    }
    Ok(())
}

fn permanent_delete(args: PathsArgs) -> Result<()> {
    let paths = checked_paths(&args.paths)?;
    let current = std::env::current_dir()?.canonicalize()?;
    for path in &paths {
        let metadata =
            fs::symlink_metadata(path).with_context(|| format!("read {}", path.display()))?;
        if metadata.is_dir() {
            let canonical = path
                .canonicalize()
                .with_context(|| format!("resolve {}", path.display()))?;
            if canonical.parent().is_none() || current.starts_with(&canonical) {
                bail!(
                    "refusing to permanently delete protected directory {}",
                    canonical.display()
                );
            }
        }
    }
    for path in paths {
        qfind_core::delete(&path)
            .with_context(|| format!("permanently delete {}", path.display()))?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string(&json!({"action":"permanently-deleted","path":path}))?
            );
        } else {
            println!("Permanently deleted {}", path.display());
        }
    }
    Ok(())
}

fn batch_transfer(
    paths: Vec<PathBuf>,
    destination: &Path,
    moving: bool,
    json_output: bool,
) -> Result<()> {
    let destination = absolute_path(destination)?;
    let paths = paths
        .iter()
        .map(|path| absolute_path(path))
        .collect::<Result<Vec<_>>>()?;
    let action = if moving { "move" } else { "copy" };
    let request = json!({"action":action,"paths":paths,"destination":destination});
    batch(request, json_output)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("paths must not contain '..': {}", path.display());
    }
    Ok(path)
}

fn batch(request: Value, json_output: bool) -> Result<()> {
    let preview = request["action"] == "rename_preview";
    let response = qfind_core::components::dispatch(
        &qfind_core::Manager::live(None),
        "batch",
        &request.to_string(),
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    let value: Value = serde_json::from_str(&response)?;
    if json_output {
        println!("{value}");
    } else {
        if let Some(items) = value["items"].as_array() {
            for item in items {
                println!(
                    "{} → {}",
                    item["from"].as_str().unwrap_or_default(),
                    item["to"].as_str().unwrap_or_default()
                );
            }
        }
        println!("{}", value["text"].as_str().unwrap_or_default());
        if preview {
            println!("Preview only; pass --apply to rename these paths.");
        }
    }
    Ok(())
}

fn archive(command: ArchiveCommand) -> Result<()> {
    match command {
        ArchiveCommand::Open { path, json } => {
            let contents = qfind_core::archive::unpack(&path).context("open archive")?;
            emit(
                json,
                json!({"action":"archive-opened","path":contents}),
                format!("Opened {}", contents.display()),
            )
        }
        ArchiveCommand::Compress {
            destination,
            paths,
            json,
        } => {
            qfind_core::archive::compress(&paths, &destination).context("compress archive")?;
            emit(
                json,
                json!({"action":"archive-created","path":destination}),
                format!("Created {}", destination.display()),
            )
        }
        ArchiveCommand::Extract {
            source,
            destination,
            json,
        } => {
            qfind_core::archive::extract(&source, &destination).context("extract archive")?;
            emit(
                json,
                json!({"action":"archive-extracted","path":destination}),
                format!("Extracted to {}", destination.display()),
            )
        }
        ArchiveCommand::Save { path, json } => {
            let workspace = qfind_core::archive::workspace(&path)
                .context("path is not an archive workspace")?;
            let source = workspace.source.clone();
            qfind_core::archive::repack(&workspace).context("save archive")?;
            emit(
                json,
                json!({"action":"archive-saved","path":source}),
                format!("Saved {}", source.display()),
            )
        }
    }
}

fn open(path: &Path) -> Result<()> {
    let config = qfind_core::Config::load();
    match config.open_how(path, path.is_dir()) {
        qfind_core::OpenHow::Editor { program, args } => {
            Command::new(&program)
                .args(args)
                .arg(path)
                .spawn()
                .with_context(|| format!("open {} with {program}", path.display()))?;
        }
        qfind_core::OpenHow::Desktop => spawn_desktop(path).context("open with desktop handler")?,
    }
    Ok(())
}

fn open_with(path: &Path, program: &str, args: &[String]) -> Result<()> {
    Command::new(program)
        .args(args)
        .arg(path)
        .spawn()
        .with_context(|| format!("open {} with {program}", path.display()))?;
    Ok(())
}

fn spawn_desktop(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(std::path::absolute(path)?)
            .spawn()?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        bail!("desktop opening is unavailable on this platform");
    }
    Ok(())
}

fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        Command::new("xdg-open").arg(target).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg("/select,")
            .arg(path)
            .spawn()?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        bail!("revealing files is unavailable on this platform");
    }
    Ok(())
}

fn preview(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        if Command::new("sushi").arg(path).spawn().is_err() {
            spawn_desktop(path).context("previewer unavailable")?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if Command::new("qlmanage")
            .arg("-p")
            .arg(path)
            .spawn()
            .is_err()
        {
            spawn_desktop(path).context("previewer unavailable")?;
        }
    }
    #[cfg(target_os = "windows")]
    {
        spawn_desktop(path).context("previewer unavailable")?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        bail!("preview is unavailable on this platform");
    }
    Ok(())
}

fn copy_name(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .context("path has no filename")?
        .to_string_lossy();
    println!("{name}");
    Ok(())
}

pub(super) fn file_uri(path: &Path) -> String {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    let raw = path.to_string_lossy().replace('\\', "/").into_bytes();
    #[cfg(not(windows))]
    let raw = path.as_os_str().as_encoded_bytes().to_vec();
    let mut uri = String::from(if cfg!(windows) {
        if raw.starts_with(b"//") {
            "file:"
        } else {
            "file:///"
        }
    } else {
        "file://"
    });
    for byte in raw {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

fn properties(path: &Path, json_output: bool) -> Result<()> {
    let link = fs::symlink_metadata(path)
        .with_context(|| format!("read properties for {}", path.display()))?;
    let target = link
        .file_type()
        .is_symlink()
        .then(|| fs::read_link(path))
        .transpose()?;
    let kind = if link.file_type().is_symlink() {
        "symlink"
    } else if link.is_dir() {
        "directory"
    } else if link.is_file() {
        "file"
    } else {
        "other"
    };
    let modified = link
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_secs());
    let size = if link.is_dir() {
        let directory = path.canonicalize()?;
        let sizes = qfind_core::FolderSizes::global();
        sizes.request(&directory);
        sizes.get(&directory)
    } else {
        Some(link.len())
    };
    let value = json!({
        "path": path,
        "kind": kind,
        "size": size,
        "modified": modified,
        "readonly": link.permissions().readonly(),
        "symlink_target": target,
    });
    if json_output {
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("Path: {}", path.display());
        println!("Type: {kind}");
        println!(
            "Size: {}",
            size.map(|bytes| format!("{bytes} bytes"))
                .unwrap_or_else(|| "not measured; use storage for indexed weights".into())
        );
        println!("Modified: {modified}");
        println!("Readonly: {}", link.permissions().readonly());
        if let Some(target) = target {
            println!("Target: {}", target.display());
        }
    }
    Ok(())
}

fn emit(json_output: bool, value: Value, text: String) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}
