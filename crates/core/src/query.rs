/// Which Hits to keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Files,
    Folders,
}

/// Filename class from extension. Folders never match a class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileClass {
    #[default]
    All,
    Image,
    Audio,
    Video,
    Document,
    Archive,
}

/// How to order Hits after a Query.
///
/// Date and size use live `stat` of the matched Hits (names-first Catalogs
/// store mtime/size as 0). Same model as a file manager: Newest / Oldest,
/// not day/week buckets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Score,
    Name,
    NameDesc,
    Newest,
    Oldest,
    Largest,
    Smallest,
}

impl Sort {
    #[must_use]
    pub fn needs_stat(self) -> bool {
        matches!(
            self,
            Self::Newest | Self::Oldest | Self::Largest | Self::Smallest
        )
    }
}

/// Keep Hits whose mtime falls in this window. Names-first Catalogs store
/// mtime `0`; those Hits always pass (unknown date).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateAge {
    #[default]
    Any,
    Day,
    Week,
    Month,
    Year,
}

/// How loose the Query is. Fuzzy is nucleo/fzf (`hlo` → `hello.txt`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchMode {
    /// Letters may have gaps. `'foo`, `^foo`, `foo$` still work (fzf syntax).
    #[default]
    Fuzzy,
    /// Contiguous substring. `ell` hits `hello.txt`; `hlo` does not.
    Substring,
    /// Whole filename, case-insensitive. `hello` misses `hello.txt`.
    Exact,
}

impl MatchMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fuzzy => "fuzzy",
            Self::Substring => "substring",
            Self::Exact => "exact",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().trim_matches('"') {
            "substring" | "strict" | "off" => Self::Substring,
            "exact" => Self::Exact,
            _ => Self::Fuzzy,
        }
    }

    #[must_use]
    pub fn cycle(self) -> Self {
        match self {
            Self::Fuzzy => Self::Substring,
            Self::Substring => Self::Exact,
            Self::Exact => Self::Fuzzy,
        }
    }
}

/// Options for [`crate::Catalog::search_with`].
#[derive(Clone, Copy, Debug)]
pub struct SearchOpts {
    pub scope: Scope,
    pub class: FileClass,
    pub sort: Sort,
    pub date: DateAge,
    pub limit: usize,
    pub highlight: bool,
    pub match_mode: MatchMode,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            scope: Scope::All,
            class: FileClass::All,
            sort: Sort::Score,
            date: DateAge::Any,
            limit: 0,
            highlight: false,
            match_mode: MatchMode::Fuzzy,
        }
    }
}

pub(crate) fn date_cutoff(age: DateAge) -> Option<i64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = match age {
        DateAge::Any => return None,
        DateAge::Day => 86_400,
        DateAge::Week => 86_400 * 7,
        DateAge::Month => 86_400 * 30,
        DateAge::Year => 86_400 * 365,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(now.saturating_sub(secs))
}

pub(crate) fn date_matches(age: DateAge, mtime: i64, cutoff: Option<i64>) -> bool {
    let Some(cut) = cutoff else {
        return true;
    };
    if age == DateAge::Any || mtime == 0 {
        return true;
    }
    mtime >= cut
}

pub(crate) fn classify(name: &str) -> FileClass {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return FileClass::All;
    };
    if ext.len() > 4 || ext.is_empty() {
        return FileClass::All;
    }
    let mut buf = [0u8; 4];
    for (i, b) in ext.as_bytes().iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    match &buf[..ext.len()] {
        b"png" | b"jpg" | b"jpeg" | b"gif" | b"webp" | b"svg" | b"bmp" | b"ico" | b"tif"
        | b"tiff" | b"heic" | b"avif" => FileClass::Image,
        b"mp3" | b"flac" | b"wav" | b"ogg" | b"m4a" | b"aac" | b"aiff" | b"opus" | b"wma" => {
            FileClass::Audio
        }
        b"mp4" | b"mkv" | b"webm" | b"mov" | b"avi" | b"m4v" => FileClass::Video,
        b"pdf" | b"doc" | b"docx" | b"odt" | b"txt" | b"md" | b"rtf" | b"xls" | b"xlsx" | b"ppt"
        | b"pptx" | b"csv" => FileClass::Document,
        b"zip" | b"tar" | b"gz" | b"bz2" | b"xz" | b"7z" | b"rar" | b"zst" => FileClass::Archive,
        _ => FileClass::All,
    }
}

pub(crate) fn class_matches(class: FileClass, name: &str, is_dir: bool) -> bool {
    if class == FileClass::All {
        return true;
    }
    if is_dir {
        return false;
    }
    classify(name) == class
}
