//! How Hits are laid out. Catalog stays names; this module is presentation math.
//!
//! GTK and TUI are two adapters. Adding a Surface means a file here plus a
//! renderer — not a change to Catalog.

mod tree;
mod treemap;
mod zoom;

pub use tree::{Flat, HitRef, Stem, fold_stems, walk_visible};
pub use treemap::{Tile, Weighted, folder_weights, squarify};
pub use zoom::{split_filename, Surface, Zoom};
