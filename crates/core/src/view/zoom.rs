/// Continuous 0–100 scale. File Pilot: Ctrl+scroll morphs list → roomy list → grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zoom(u8);

/// Which Hits surface is showing. `Auto` follows [`Zoom`] (list below 40, grid from 40).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Auto,
    Tree,
}

impl Default for Zoom {
    fn default() -> Self {
        Self(12)
    }
}

impl Zoom {
    /// Grid starts here, matching File Pilot’s “small icons ~30–40%”.
    pub const GRID_FROM: u8 = 40;

    #[must_use]
    pub fn new(v: u8) -> Self {
        Self(v.min(100))
    }

    #[must_use]
    pub fn get(self) -> u8 {
        self.0
    }

    #[must_use]
    pub fn bump(self, steps: i8) -> Self {
        let v = i16::from(self.0) + i16::from(steps) * 5;
        Self(v.clamp(0, 100) as u8)
    }

    #[must_use]
    pub fn is_grid(self) -> bool {
        self.0 >= Self::GRID_FROM
    }

    /// List row height. One Ctrl+scroll notch (~5 Zoom) is ~10px so it reads as size, not pad.
    #[must_use]
    pub fn row_px(self) -> i32 {
        let t = i32::from(self.0.min(Self::GRID_FROM - 1));
        24 + t * 2
    }

    #[must_use]
    pub fn pad_px(self) -> i32 {
        self.pad_px_with(0)
    }

    #[must_use]
    pub fn pad_px_with(self, extra: u8) -> i32 {
        let t = i32::from(self.0.min(Self::GRID_FROM - 1));
        2 + t / 5 + i32::from(extra)
    }

    #[must_use]
    pub fn icon_px(self) -> i32 {
        if self.is_grid() {
            (self.cell_px() - 22).max(32)
        } else {
            16 + i32::from(self.0.min(Self::GRID_FROM - 1)) * 5 / 4
        }
    }

    /// Square tile edge. Columns = floor(width / cell_px), leftover pixels stretch the row.
    #[must_use]
    pub fn cell_px(self) -> i32 {
        96 + i32::from(self.0.saturating_sub(Self::GRID_FROM)) * 4
    }

    #[must_use]
    pub fn columns_for(self, width_px: i32) -> u32 {
        (width_px.max(1) / self.cell_px()).max(1) as u32
    }
}

/// Stem + extension (`wav` without the dot). Folders and dotfiles have an empty ext so the
/// whole name stays in the stem. The Hits surface pins `.{ext}` and marquees the stem.
#[must_use]
pub fn split_filename(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 10
                && ext.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            (stem, ext)
        }
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morphs_list_to_grid() {
        assert!(!Zoom::new(0).is_grid());
        assert!(!Zoom::new(39).is_grid());
        assert!(Zoom::new(40).is_grid());
        assert!(Zoom::new(100).is_grid());
        assert!(Zoom::new(10).row_px() < Zoom::new(30).row_px());
        assert!(Zoom::new(0).icon_px() < Zoom::new(30).icon_px());
        assert!(Zoom::new(12).row_px() + 8 <= Zoom::new(17).row_px());
        assert!(Zoom::new(50).cell_px() < Zoom::new(90).cell_px());
        assert_eq!(Zoom::new(40).columns_for(960), 10);
        assert_eq!(Zoom::new(40).columns_for(0), 1);
    }

    #[test]
    fn split_keeps_extension() {
        assert_eq!(split_filename("song.wav", false), ("song", "wav"));
        assert_eq!(split_filename("setup.exe", false), ("setup", "exe"));
        assert_eq!(split_filename("a.tar.gz", false), ("a.tar", "gz"));
        assert_eq!(split_filename(".gitignore", false), (".gitignore", ""));
        assert_eq!(split_filename("Photos", true), ("Photos", ""));
        assert_eq!(split_filename("README", false), ("README", ""));
    }

    #[test]
    fn bump_clamps() {
        assert_eq!(Zoom::new(0).bump(-1).get(), 0);
        assert_eq!(Zoom::new(100).bump(1).get(), 100);
        assert_eq!(Zoom::new(10).bump(1).get(), 15);
    }
}
