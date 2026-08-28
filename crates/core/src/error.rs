use std::io;
use std::path::PathBuf;

/// Failures at the Catalog interface.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("io error: {0}")]
    IoOther(#[from] io::Error),
    #[error("invalid snapshot {path}: {reason}")]
    Snapshot { path: PathBuf, reason: &'static str },
    #[error("invalid query: {0}")]
    Query(String),
    #[error("exclude pattern {pattern}: {source}")]
    Exclude {
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
