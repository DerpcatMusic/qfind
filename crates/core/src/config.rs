//! User Config: include/exclude Mounts, Zoom, spacing, PreviewMode.
//! Stored at `$XDG_CONFIG_HOME/qfind/config.toml`.

use std::path::PathBuf;

use crate::catalog::Rebuild;
use crate::query::MatchMode;
use crate::view::Zoom;
use crate::{default_snapshot_path};

/// Whether Space previews the hovered Hit or the selected Hit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    #[default]
    Hovered,
    Selected,
}

impl PreviewMode {
    fn as_str(self) -> &'static str {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Extra Exclude names/globs on top of the built-in junk list.
    pub exclude: Vec<String>,
    /// Mounts to Rebuild. Empty = discover local Mounts.
    pub include: Vec<PathBuf>,
    pub zoom: u8,
    /// Extra row padding in pixels (0–24).
    pub spacing: u8,
    pub preview: PreviewMode,
    pub zebra: bool,
    pub weight_map: bool,
    pub match_mode: MatchMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            include: Vec::new(),
            zoom: Zoom::default().get(),
            spacing: 0,
            preview: PreviewMode::Hovered,
            zebra: true,
            weight_map: true,
            match_mode: MatchMode::Fuzzy,
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
        let mut s = String::from("# Qfind Config — reset by deleting this file or Settings → Reset\n");
        s.push_str("exclude = [");
        s.push_str(&toml_list(&self.exclude));
        s.push_str("]\ninclude = [");
        let inc: Vec<String> = self
            .include
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        s.push_str(&toml_list(&inc));
        s.push_str("]\n");
        s.push_str(&format!("zoom = {}\n", self.zoom.min(100)));
        s.push_str(&format!("spacing = {}\n", self.spacing.min(24)));
        s.push_str(&format!("preview = \"{}\"\n", self.preview.as_str()));
        s.push_str(&format!("zebra = {}\n", self.zebra));
        s.push_str(&format!("weight_map = {}\n", self.weight_map));
        s.push_str(&format!("match = \"{}\"\n", self.match_mode.as_str()));
        s
    }

    #[must_use]
    pub fn rebuild(&self) -> Rebuild {
        let mut r = Rebuild::new(default_snapshot_path());
        if !self.include.is_empty() {
            r = r.roots(self.include.clone());
        }
        for e in &self.exclude {
            r = r.exclude(e);
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
            "include" => cfg.include = parse_list(v).into_iter().map(PathBuf::from).collect(),
            "zoom" => cfg.zoom = v.parse().unwrap_or(cfg.zoom).min(100),
            "spacing" => cfg.spacing = v.parse().unwrap_or(cfg.spacing).min(24),
            "preview" => cfg.preview = PreviewMode::parse(v.trim_matches('"')),
            "zebra" => cfg.zebra = v == "true",
            "weight_map" => cfg.weight_map = v == "true",
            "match" => cfg.match_mode = MatchMode::parse(v),
            _ => {}
        }
    }
    cfg
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
        let again = parse(&cfg.to_toml());
        assert_eq!(again.exclude, cfg.exclude);
        assert_eq!(again.include, cfg.include);
        assert_eq!(again.spacing, 8);
        assert_eq!(again.preview, PreviewMode::Selected);
        assert!(!again.zebra);
        assert_eq!(again.match_mode, MatchMode::Substring);
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
