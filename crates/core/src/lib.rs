//! Qfind Catalog: one module for Rebuild and Query.
//!
//! Walk, packed snapshot, Excludes, and Query parsing are implementation.

mod catalog;
mod config;
mod error;
mod exclude;
mod mounts;
mod prefilter;
mod query;
mod search;
mod snapshot;
mod view;
mod walk;

pub use catalog::{Catalog, Hit, Hits, Rebuild, default_snapshot_path};
pub use config::{Config, OpenHow, OpenMode, PreviewMode, is_text_path};
pub use error::{Error, Result};
pub use query::{DateAge, FileClass, MatchMode, Scope, SearchOpts, Sort};
pub use view::{
    Flat, HitRef, Stem, Surface, Tile, Weighted, Zoom, fold_stems, folder_weights, split_filename,
    squarify, walk_visible,
};
