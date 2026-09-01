//! Qfind Catalog: one module for Rebuild and Query.
//!
//! Walk, packed snapshot, Excludes, and Query parsing are implementation.

mod catalog;
mod config;
mod error;
mod exclude;
mod manager;
mod mounts;
mod prefilter;
mod query;
mod search;
mod snapshot;
mod storage;
mod view;
mod walk;

pub use catalog::{Catalog, CatalogFolder, Hit, Hits, Rebuild, default_snapshot_path};
pub use config::{Config, IgnoreMatcher, OpenHow, OpenMode, PreviewMode, is_text_path};
pub use error::{Error, Result};
pub use manager::{
    BrowseMode, ChartScope, LocationScope, Manager, ManagerRow, ManagerSession, ManagerView,
};
pub use query::{DateAge, FileClass, MatchMode, Scope, SearchOpts, Sort};
pub use storage::{StorageEntry, StorageMap};
pub use view::{
    Flat, HitRef, Stem, Surface, Tile, Weighted, Zoom, fold_stems, folder_weights, split_filename,
    squarify, walk_visible,
};
