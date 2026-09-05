//! Qfind Catalog: one module for Rebuild and Query.
//!
//! Walk, packed snapshot, Excludes, and Query parsing are implementation.

#[cfg(feature = "archives")]
pub mod archive;
pub mod components;
pub mod folder_sizes;
pub use folder_sizes::FolderSizes;
mod process;
pub mod projects;
mod browse;
mod catalog;
mod config;
mod error;
mod exclude;
mod manager;
mod mounts;
mod nav;
mod ops;
mod plugin;
mod prefilter;
mod query;
mod search;
mod snapshot;
mod storage;
mod view;
mod walk;

pub use browse::{LiveEntry, live_children};
pub use catalog::{Catalog, CatalogFolder, Hit, Hits, Rebuild, default_snapshot_path};
pub use config::{Config, IgnoreMatcher, OpenHow, OpenMode, PreviewMode, is_text_path};
pub use error::{Error, Result};
pub use manager::{
    Action, BrowseMode, ChartScope, LocationScope, Manager, ManagerRow, ManagerSession,
    ManagerView, Outcome,
};
pub use nav::{Crumb, Location, TreeState, breadcrumb};
pub use ops::{
    Mutation, copy, create_dir, create_file, delete, delete_entry, move_path, rename, restore,
    trash, trash_entry, trash_into, trash_root,
};
pub use plugin::{Plugin, PluginHost};
pub use query::{DateAge, FileClass, MatchMode, Scope, SearchOpts, Sort};
pub use storage::{StorageEntry, StorageMap};
pub use view::{
    Flat, HitRef, Stem, Surface, Tile, Weighted, Zoom, fold_stems, folder_weights, split_filename,
    squarify, walk_visible,
};
