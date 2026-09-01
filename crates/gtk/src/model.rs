use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gio::subclass::prelude::ListModelImpl;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use qfind_core::Catalog;

use crate::row::RowData;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct HitModel {
        pub catalog: RefCell<Option<Catalog>>,
        pub ids: RefCell<Vec<u32>>,
        pub live: RefCell<Option<Vec<RowData>>>,
        /// Keep RowData GObjects across scroll. Zed: don't rebuild visible items every frame.
        pub rows: RefCell<HashMap<u32, RowData>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HitModel {
        const NAME: &'static str = "QfindHitModel";
        type Type = super::HitModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for HitModel {}

    impl ListModelImpl for HitModel {
        fn item_type(&self) -> glib::Type {
            RowData::static_type()
        }

        fn n_items(&self) -> u32 {
            self.live
                .borrow()
                .as_ref()
                .map_or_else(|| self.ids.borrow().len(), Vec::len) as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            if let Some(rows) = self.live.borrow().as_ref() {
                return rows.get(position as usize).cloned().map(|row| row.upcast());
            }
            let ids = self.ids.borrow();
            let id = *ids.get(position as usize)?;
            drop(ids);
            if let Some(row) = self.rows.borrow().get(&id) {
                return Some(row.clone().upcast());
            }
            let catalog = self.catalog.borrow();
            let catalog = catalog.as_ref()?;
            let hit = catalog.hit(id)?;
            let row = RowData::new(
                hit.name(),
                hit.path().to_string_lossy().into_owned(),
                hit.is_dir(),
            );
            self.rows.borrow_mut().insert(id, row.clone());
            Some(row.upcast())
        }
    }
}

glib::wrapper! {
    pub struct HitModel(ObjectSubclass<imp::HitModel>)
        @implements gio::ListModel;
}

impl HitModel {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_catalog(&self, catalog: Catalog) {
        self.imp().catalog.replace(Some(catalog));
    }

    pub fn set_ids(&self, ids: Vec<u32>) {
        let old = self.n_items();
        let new = ids.len() as u32;
        {
            let keep: HashSet<u32> = ids.iter().copied().collect();
            let mut cache = self.imp().rows.borrow_mut();
            cache.retain(|id, _| keep.contains(id));
            if cache.len() > 512 {
                cache.clear();
            }
        }
        self.imp().live.replace(None);
        self.imp().ids.replace(ids);
        self.items_changed(0, old, new);
    }

    pub fn set_rows(&self, rows: Vec<RowData>) {
        let old = self.n_items();
        let new = rows.len() as u32;
        self.imp().ids.borrow_mut().clear();
        self.imp().rows.borrow_mut().clear();
        self.imp().live.replace(Some(rows));
        self.items_changed(0, old, new);
    }

    pub fn id(&self, position: u32) -> Option<u32> {
        self.imp().ids.borrow().get(position as usize).copied()
    }

    pub fn position(&self, id: u32) -> Option<u32> {
        self.imp()
            .ids
            .borrow()
            .iter()
            .position(|candidate| *candidate == id)
            .and_then(|position| u32::try_from(position).ok())
    }

    pub fn position_path(&self, path: &str) -> Option<u32> {
        self.imp()
            .live
            .borrow()
            .as_ref()?
            .iter()
            .position(|row| row.path() == path)
            .and_then(|position| u32::try_from(position).ok())
    }
}

impl Default for HitModel {
    fn default() -> Self {
        Self::new()
    }
}
