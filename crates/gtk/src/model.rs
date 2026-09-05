use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
        pub text_sort: Cell<Option<(bool, bool)>>,
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
                hit.size(),
                hit.mtime(),
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

    pub fn set_ids(&self, mut ids: Vec<u32>) {
        self.sort_ids(&mut ids);
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

    pub fn set_rows(&self, mut rows: Vec<RowData>) {
        self.sort_rows(&mut rows);
        let old = self.n_items();
        let new = rows.len() as u32;
        self.imp().ids.borrow_mut().clear();
        self.imp().rows.borrow_mut().clear();
        self.imp().live.replace(Some(rows));
        self.items_changed(0, old, new);
    }

    /// Supplementary columns sort loaded results without losing catalog identity.
    pub fn set_text_sort(&self, sort: Option<(bool, bool)>) {
        if self.imp().text_sort.replace(sort) == sort || sort.is_none() { return; }
        if let Some(rows) = self.imp().live.borrow_mut().as_mut() { self.sort_rows(rows); }
        else { self.sort_ids(&mut self.imp().ids.borrow_mut()); }
        let count = self.n_items();
        self.items_changed(0, count, count);
    }

    fn sort_ids(&self, ids: &mut Vec<u32>) {
        let Some((location, descending)) = self.imp().text_sort.get() else { return; };
        let catalog = self.imp().catalog.borrow();
        let Some(catalog) = catalog.as_ref() else { return; };
        sort_text(ids, descending, |id| catalog.hit(*id).map(|hit| {
            let path = hit.path();
            (crate::FOLDERS_FIRST.load(std::sync::atomic::Ordering::Relaxed) && !hit.is_dir(),
                crate::columns::text_key(hit.name(), &path, hit.is_dir(), location), path.to_string_lossy().into_owned())
        }).unwrap_or_default());
    }

    fn sort_rows(&self, rows: &mut Vec<RowData>) {
        let Some((location, descending)) = self.imp().text_sort.get() else { return; };
        sort_text(rows, descending, |row| (crate::FOLDERS_FIRST.load(std::sync::atomic::Ordering::Relaxed) && !row.is_dir(),
            crate::columns::text_value(row, location), row.path()));
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
        if let Some(rows) = self.imp().live.borrow().as_ref() {
            return rows
                .iter()
                .position(|row| row.path() == path)
                .and_then(|position| u32::try_from(position).ok());
        }
        let catalog = self.imp().catalog.borrow();
        let catalog = catalog.as_ref()?;
        self.imp()
            .ids
            .borrow()
            .iter()
            .position(|id| {
                catalog
                    .hit(*id)
                    .is_some_and(|hit| hit.path() == Path::new(path))
            })
            .and_then(|position| u32::try_from(position).ok())
    }
}

impl Default for HitModel {
    fn default() -> Self {
        Self::new()
    }
}

fn sort_text<T>(items: &mut Vec<T>, descending: bool, key: impl Fn(&T) -> (bool, String, String)) {
    let mut keyed: Vec<_> = items.drain(..).map(|item| (key(&item), item)).collect();
    keyed.sort_by(|(a, _), (b, _)| a.0.cmp(&b.0).then_with(|| {
        let order = a.1.cmp(&b.1);
        if descending { order.reverse() } else { order }
    }).then_with(|| a.2.cmp(&b.2)));
    items.extend(keyed.into_iter().map(|(_, item)| item));
}
