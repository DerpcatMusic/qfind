//! User Config: include/exclude Mounts, Zoom, spacing, PreviewMode.
//! Stored at `$XDG_CONFIG_HOME/qfind/config.toml`.

use std::path::{Path, PathBuf};

use crate::catalog::Rebuild;
use crate::default_snapshot_path;
use crate::query::MatchMode;
use crate::view::Zoom;

/// Lazily loads hierarchical Git and ripgrep ignore rules for queried paths.
pub struct IgnoreMatcher(ignore::IncrementalIgnore);

impl IgnoreMatcher {
    #[must_use]
    pub fn new(respect_gitignore: bool, respect_ignore: bool) -> Option<Self> {
        if !respect_gitignore && !respect_ignore {
            return None;
        }
        let mut builder = ignore::WalkBuilder::new(Path::new("/"));
        builder
            .standard_filters(false)
            .hidden(false)
            .parents(true)
            .ignore(respect_ignore)
            .git_ignore(respect_gitignore)
            .git_global(respect_gitignore)
            .git_exclude(respect_gitignore)
            .require_git(true)
            .follow_links(false);
        builder.build_matchers().pop().map(Self)
    }

    pub fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        let relative = path.strip_prefix(self.0.root()).unwrap_or(path);
        self.0.matched(relative, is_dir).is_ignore()
    }
}

/// Whether Space previews the hovered Hit or the selected Hit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    #[default]
    Hovered,
    Selected,
}

impl PreviewMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hovered => "hovered",
            Self::Selected => "selected",
        }
    }

    fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("selected") {
            Self::Selected
        } else {
            Self::Hovered
        }
    }
}

/// How Enter opens a Hit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenMode {
    /// `$EDITOR` / `$VISUAL` (or [`Config::editor`]) for text; desktop handler otherwise.
    #[default]
    Auto,
    /// Always `xdg-open` / the desktop MIME default.
    Xdg,
    /// Always the editor. Falls back to the desktop handler if none is set.
    Editor,
}

impl OpenMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Xdg => "xdg",
            Self::Editor => "editor",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().trim_matches('"') {
            "xdg" | "desktop" => Self::Xdg,
            "editor" => Self::Editor,
            _ => Self::Auto,
        }
    }

    #[must_use]
    pub fn cycle(self) -> Self {
        match self {
            Self::Auto => Self::Xdg,
            Self::Xdg => Self::Editor,
            Self::Editor => Self::Auto,
        }
    }
}

/// Where to send a Hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenHow {
    Desktop,
    Editor { program: String, args: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Extra Exclude names/globs on top of the built-in junk list.
    pub exclude: Vec<String>,
    /// Exact directory trees excluded from the Catalog.
    pub exclude_paths: Vec<PathBuf>,
    /// Mounts to Rebuild. Empty = discover local Mounts.
    pub include: Vec<PathBuf>,
    /// Show dotfiles and entries below dot-directories in Query results.
    pub show_hidden: bool,
    /// Respect `.gitignore`, Git's global excludes, and `.git/info/exclude`.
    pub respect_gitignore: bool,
    /// Respect ripgrep-style `.ignore` files.
    pub respect_ignore: bool,
    pub zoom: u8,
    /// Extra row padding in pixels (0–24).
    pub spacing: u8,
    pub preview: PreviewMode,
    /// Side preview width as a percentage of the Hits surface (20–70).
    pub preview_width: u8,
    pub zebra: bool,
    pub weight_map: bool,
    pub match_mode: MatchMode,
    /// TUI theme id: grok, titanium, catppuccin, gruvbox, dracula, nord, aurora.
    pub theme: String,
    /// Deprecated compatibility setting. Startup now opens immediately.
    pub splash: bool,
    /// How Enter opens a Hit.
    pub open: OpenMode,
    /// Editor binary + args. Empty = `$EDITOR`, then `$VISUAL`.
    pub editor: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            exclude_paths: Vec::new(),
            include: Vec::new(),
            show_hidden: true,
            respect_gitignore: false,
            respect_ignore: false,
            zoom: Zoom::default().get(),
            spacing: 0,
            preview: PreviewMode::Hovered,
            preview_width: 36,
            zebra: true,
            weight_map: true,
            match_mode: MatchMode::Fuzzy,
            theme: "grok".into(),
            splash: false,
            open: OpenMode::Auto,
            editor: String::new(),
        }
    }
}

impl Config {
    #[must_use]
    pub fn path() -> PathBuf {
        let dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.join("qfind").join("config.toml")
    }

    #[must_use]
    pub fn load() -> Self {
        let path = Self::path();
        std::fs::read_to_string(&path)
            .ok()
            .map(|s| parse(&s))
            .unwrap_or_default()
    }

    /// # Errors
    /// Returns IO errors from creating the config directory or writing the file.
    pub fn save(&self) -> crate::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| crate::Error::io(dir, e))?;
        }
        std::fs::write(&path, self.to_toml()).map_err(|e| crate::Error::io(&path, e))?;
        Ok(())
    }

    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut s =
            String::from("# Qfind Config — reset by deleting this file or Settings → Reset\n");
        s.push_str("exclude = [");
        s.push_str(&toml_list(&self.exclude));
        s.push_str("]\nexclude_paths = [");
        let excluded: Vec<String> = self
            .exclude_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        s.push_str(&toml_list(&excluded));
        s.push_str("]\ninclude = [");
        let inc: Vec<String> = self
            .include
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        s.push_str(&toml_list(&inc));
        s.push_str("]\n");
        s.push_str(&format!("show_hidden = {}\n", self.show_hidden));
        s.push_str(&format!("respect_gitignore = {}\n", self.respect_gitignore));
        s.push_str(&format!("respect_ignore = {}\n", self.respect_ignore));
        s.push_str(&format!("zoom = {}\n", self.zoom.min(100)));
        s.push_str(&format!("spacing = {}\n", self.spacing.min(24)));
        s.push_str(&format!("preview = \"{}\"\n", self.preview.as_str()));
        s.push_str(&format!(
            "preview_width = {}\n",
            self.preview_width.clamp(20, 70)
        ));
        s.push_str(&format!("zebra = {}\n", self.zebra));
        s.push_str(&format!("weight_map = {}\n", self.weight_map));
        s.push_str(&format!("match = \"{}\"\n", self.match_mode.as_str()));
        s.push_str(&format!("theme = \"{}\"\n", self.theme));
        s.push_str(&format!("splash = {}\n", self.splash));
        s.push_str(&format!("open = \"{}\"\n", self.open.as_str()));
        s.push_str(&format!(
            "editor = \"{}\"\n",
            self.editor.replace('"', "\\\"")
        ));
        s
    }

    /// Decide desktop handler vs editor for this Hit.
    #[must_use]
    pub fn open_how(&self, path: &Path, is_dir: bool) -> OpenHow {
        self.open_how_env(
            path,
            is_dir,
            std::env::var("EDITOR").ok().as_deref(),
            std::env::var("VISUAL").ok().as_deref(),
        )
    }

    fn open_how_env(
        &self,
        path: &Path,
        is_dir: bool,
        env_editor: Option<&str>,
        env_visual: Option<&str>,
    ) -> OpenHow {
        let editor = self.editor_cmd(env_editor, env_visual);
        match self.open {
            OpenMode::Xdg => OpenHow::Desktop,
            OpenMode::Editor => editor.unwrap_or(OpenHow::Desktop),
            OpenMode::Auto => {
                if is_dir || !is_text_path(path) {
                    OpenHow::Desktop
                } else {
                    editor.unwrap_or(OpenHow::Desktop)
                }
            }
        }
    }

    fn editor_cmd(&self, env_editor: Option<&str>, env_visual: Option<&str>) -> Option<OpenHow> {
        let raw = if !self.editor.trim().is_empty() {
            self.editor.as_str()
        } else if let Some(e) = env_editor.filter(|s| !s.trim().is_empty()) {
            e
        } else {
            env_visual.filter(|s| !s.trim().is_empty())?
        };
        split_cmd(raw).map(|(program, args)| OpenHow::Editor { program, args })
    }

    #[must_use]
    pub fn rebuild(&self) -> Rebuild {
        self.rebuild_to(default_snapshot_path())
    }

    #[must_use]
    pub fn rebuild_to(&self, snapshot: impl Into<PathBuf>) -> Rebuild {
        let mut r = Rebuild::new(snapshot);
        if !self.include.is_empty() {
            r = r.roots(self.include.clone());
        }
        for e in &self.exclude {
            r = r.exclude(e);
        }
        for path in &self.exclude_paths {
            r = r.exclude_path(path);
        }
        r
    }
}

fn toml_list(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("\"{}\"", i.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse(src: &str) -> Config {
    let mut cfg = Config::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        match k {
            "exclude" => cfg.exclude = parse_list(v),
            "exclude_paths" => {
                cfg.exclude_paths = parse_list(v).into_iter().map(PathBuf::from).collect();
            }
            "include" => cfg.include = parse_list(v).into_iter().map(PathBuf::from).collect(),
            "show_hidden" => cfg.show_hidden = v != "false",
            "respect_gitignore" => cfg.respect_gitignore = v == "true",
            "respect_ignore" => cfg.respect_ignore = v == "true",
            "zoom" => cfg.zoom = v.parse().unwrap_or(cfg.zoom).min(100),
            "spacing" => cfg.spacing = v.parse().unwrap_or(cfg.spacing).min(24),
            "preview" => cfg.preview = PreviewMode::parse(v.trim_matches('"')),
            "preview_width" => {
                cfg.preview_width = v.parse().unwrap_or(cfg.preview_width).clamp(20, 70);
            }
            "zebra" => cfg.zebra = v == "true",
            "weight_map" => cfg.weight_map = v == "true",
            "match" => cfg.match_mode = MatchMode::parse(v),
            "theme" => cfg.theme = v.trim_matches('"').to_string(),
            "splash" => cfg.splash = v != "false",
            "open" => cfg.open = OpenMode::parse(v),
            "editor" => cfg.editor = v.trim_matches('"').to_string(),
            _ => {}
        }
    }
    cfg
}

fn split_cmd(s: &str) -> Option<(String, Vec<String>)> {
    let mut parts = s.split_whitespace();
    let program = parts.next()?.to_string();
    if program.is_empty() {
        return None;
    }
    Some((program, parts.map(str::to_string).collect()))
}

/// Source, config, and other files `$EDITOR` should get. Not PDFs or images.
#[must_use]
pub fn is_text_path(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.is_empty() {
        return false;
    }
    if matches!(
        name,
        "Makefile"
            | "makefile"
            | "GNUmakefile"
            | "Dockerfile"
            | "Dockerfile.dev"
            | "CMakeLists.txt"
            | "Cargo.lock"
            | "Gemfile"
            | "Rakefile"
            | "Justfile"
            | "justfile"
            | "README"
            | "LICENSE"
            | "COPYING"
            | "CHANGELOG"
            | "TODO"
    ) {
        return true;
    }
    if name.starts_with('.') && !name[1..].contains('.') {
        return true;
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "rs" | "toml"
            | "md"
            | "txt"
            | "py"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "json"
            | "yml"
            | "yaml"
            | "c"
            | "h"
            | "cc"
            | "hh"
            | "cpp"
            | "hpp"
            | "cs"
            | "go"
            | "rb"
            | "php"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "css"
            | "scss"
            | "html"
            | "htm"
            | "xml"
            | "ini"
            | "cfg"
            | "conf"
            | "nix"
            | "lua"
            | "vim"
            | "sql"
            | "csv"
            | "tsv"
            | "log"
            | "lock"
            | "gradle"
            | "cmake"
            | "mk"
            | "r"
            | "swift"
            | "kt"
            | "kts"
            | "scala"
            | "hs"
            | "erl"
            | "ex"
            | "exs"
            | "clj"
            | "lisp"
            | "el"
            | "pl"
            | "pm"
            | "raku"
            | "jl"
            | "zig"
            | "v"
            | "vue"
            | "svelte"
            | "astro"
            | "tex"
            | "bib"
            | "org"
            | "adoc"
            | "rst"
            | "patch"
            | "diff"
            | "gitignore"
            | "dockerignore"
            | "editorconfig"
            | "env"
            | "service"
            | "desktop"
    )
}

fn parse_list(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches('[').trim_end_matches(']');
    if v.is_empty() {
        return Vec::new();
    }
    v.split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_lists_and_preview() {
        let mut cfg = Config::default();
        cfg.exclude = vec!["SteamLibrary".into(), "node_modules".into()];
        cfg.include = vec![PathBuf::from("/home/a")];
        cfg.spacing = 8;
        cfg.preview = PreviewMode::Selected;
        cfg.zebra = false;
        cfg.match_mode = MatchMode::Substring;
        cfg.theme = "catppuccin".into();
        cfg.splash = false;
        let again = parse(&cfg.to_toml());
        assert_eq!(again.exclude, cfg.exclude);
        assert_eq!(again.include, cfg.include);
        assert_eq!(again.spacing, 8);
        assert_eq!(again.preview, PreviewMode::Selected);
        assert!(!again.zebra);
        assert_eq!(again.match_mode, MatchMode::Substring);
        assert_eq!(again.theme, "catppuccin");
        assert!(!again.splash);
        cfg.open = OpenMode::Editor;
        cfg.editor = "helix".into();
        let again = parse(&cfg.to_toml());
        assert_eq!(again.open, OpenMode::Editor);
        assert_eq!(again.editor, "helix");
    }

    #[test]
    fn auto_sends_text_to_editor_and_binaries_to_desktop() {
        let mut cfg = Config::default();
        cfg.editor = "nvim -p".into();
        let how = cfg.open_how_env(Path::new("/tmp/foo.rs"), false, None, None);
        assert_eq!(
            how,
            OpenHow::Editor {
                program: "nvim".into(),
                args: vec!["-p".into()],
            }
        );
        assert_eq!(
            cfg.open_how_env(Path::new("/tmp/shot.png"), false, None, None),
            OpenHow::Desktop
        );
        assert_eq!(
            cfg.open_how_env(Path::new("/tmp"), true, None, None),
            OpenHow::Desktop
        );
    }

    #[test]
    fn xdg_ignores_editor_even_for_text() {
        let mut cfg = Config::default();
        cfg.open = OpenMode::Xdg;
        cfg.editor = "nvim".into();
        assert_eq!(
            cfg.open_how_env(Path::new("main.rs"), false, Some("nvim"), None),
            OpenHow::Desktop
        );
    }

    #[test]
    fn env_editor_used_when_config_editor_empty() {
        let cfg = Config::default();
        let how = cfg.open_how_env(Path::new(".bashrc"), false, Some("kak"), Some("gvim"));
        assert_eq!(
            how,
            OpenHow::Editor {
                program: "kak".into(),
                args: vec![],
            }
        );
    }

    #[test]
    fn missing_editor_falls_back_to_desktop() {
        let cfg = Config::default();
        assert_eq!(
            cfg.open_how_env(Path::new("lib.rs"), false, None, None),
            OpenHow::Desktop
        );
    }

    #[test]
    fn default_is_hovered_and_discover_roots() {
        let c = Config::default();
        assert!(c.include.is_empty());
        assert_eq!(c.preview, PreviewMode::Hovered);
        assert_eq!(c.spacing, 0);
    }

    #[test]
    fn path_is_under_config_dir() {
        assert!(Config::path().ends_with(std::path::Path::new("qfind/config.toml")));
    }
}
