use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RowData {
        pub name: RefCell<String>,
        pub path: RefCell<String>,
        pub is_dir: Cell<bool>,
        pub depth: Cell<u32>,
        pub has_kids: Cell<bool>,
        pub size: Cell<u64>,
        /// Unix seconds, `0` when unknown.
        pub modified: Cell<i64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowData {
        const NAME: &'static str = "QfindRowData";
        type Type = super::RowData;
    }

    impl ObjectImpl for RowData {}
}

glib::wrapper! {
    pub struct RowData(ObjectSubclass<imp::RowData>);
}

impl RowData {
    pub fn new(
        name: impl Into<String>,
        path: impl Into<String>,
        is_dir: bool,
        size: u64,
        modified: i64,
    ) -> Self {
        let obj = Self::with_depth(name, path, is_dir, 0);
        obj.imp().size.set(size);
        obj.imp().modified.set(modified);
        obj
    }

    pub fn with_depth(
        name: impl Into<String>,
        path: impl Into<String>,
        is_dir: bool,
        depth: u32,
    ) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().name.replace(name.into());
        obj.imp().path.replace(path.into());
        obj.imp().is_dir.set(is_dir);
        obj.imp().depth.set(depth);
        obj.imp().has_kids.set(false);
        obj.imp().size.set(0);
        obj.imp().modified.set(0);
        obj
    }

    pub fn with_fold(
        name: impl Into<String>,
        path: impl Into<String>,
        is_dir: bool,
        depth: u32,
        has_kids: bool,
    ) -> Self {
        let obj = Self::with_depth(name, path, is_dir, depth);
        obj.imp().has_kids.set(has_kids);
        obj
    }

    pub fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }

    pub fn is_dir(&self) -> bool {
        self.imp().is_dir.get()
    }

    pub fn depth(&self) -> u32 {
        self.imp().depth.get()
    }

    pub fn has_kids(&self) -> bool {
        self.imp().has_kids.get()
    }

    pub fn size(&self) -> u64 {
        self.imp().size.get()
    }

    pub fn modified(&self) -> i64 {
        self.imp().modified.get()
    }

    /// Cache live filesystem metadata (see the list factory's lazy stat).
    pub fn fill_metadata(&self, size: u64, modified: i64) {
        self.imp().size.set(size);
        self.imp().modified.set(modified);
    }
}
