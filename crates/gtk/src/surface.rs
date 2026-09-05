//! Hits Surface adapter: list, grid, tree, WeightMap. Preview keys live here,
//! not on the window — Query typing must keep Space.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use qfind_core::{
    Catalog, HitRef, PreviewMode, Surface, Tile, Weighted, Zoom, fold_stems, folder_weights,
    squarify, walk_visible,
};

use crate::actions::{content_for_path, content_for_paths, preview, selected_row, selected_rows};
use crate::row::RowData;

pub struct Host {
    #[allow(dead_code)]
    pub root: gtk::Box,
    pub stack: gtk::Stack,
    pub list: gtk::ColumnView,
    pub grid: gtk::GridView,
    #[allow(dead_code)]
    pub tree: gtk::ListView,
    pub tree_store: gio::ListStore,
    pub weight: gtk::DrawingArea,
    pub weight_rev: Rc<Cell<u64>>,
    pub zoom_label: gtk::Label,
    pub zoom_scale: gtk::Scale,
    pub apply_pending: Cell<bool>,
    pub zoom: Rc<Cell<Zoom>>,
    pub spacing: Rc<Cell<u8>>,
    pub surface: Rc<Cell<Surface>>,
    pub show_weight: Rc<Cell<bool>>,
    pub collapsed: Rc<RefCell<HashSet<String>>>,
    pub tree_src: RefCell<Option<(Catalog, Vec<u32>)>>,
    pub weights: Rc<RefCell<Vec<Weighted>>>,
}

impl Host {
    pub fn schedule_apply(self: &Rc<Self>) {
        if self.apply_pending.replace(true) { return; }
        let host = self.clone();
        self.stack.add_tick_callback(move |_, _| {
            host.apply_pending.set(false);
            host.apply();
            glib::ControlFlow::Break
        });
    }

    pub fn apply(&self) {
        let zoom = self.zoom.get();
        let surface = self.surface.get();
        let name = match surface {
            Surface::Tree => "tree",
            Surface::Auto if zoom.is_grid() => "grid",
            Surface::Auto => "list",
        };
        self.stack.set_visible_child_name(name);
        if self.zoom_scale.value() as u8 != zoom.get() {
            self.zoom_scale.set_value(f64::from(zoom.get()));
        }
        self.zoom_label.set_text(&format!("{}%", zoom.get()));
        let show_weight = self.show_weight.get();
        self.weight.set_visible(show_weight);
        if show_weight {
            self.weight.queue_draw();
        }
        self.fit_grid();
        match name {
            "list" => {
                restyle_list(&self.list, zoom, self.spacing.get());
                self.list.queue_resize();
            }
            "grid" => {
                restyle_grid(&self.grid, zoom);
                self.grid.queue_resize();
            }
            _ => {}
        }
    }

    pub fn fit_grid(&self) {
        let zoom = self.zoom.get();
        if !zoom.is_grid() {
            return;
        }
        let w = self.grid.width();
        if w <= 1 {
            return;
        }
        let cols = (w / (grid_cell_px(zoom) + 12)).max(1) as u32;
        if self.grid.min_columns() != cols || self.grid.max_columns() != cols {
            self.grid.set_min_columns(cols);
            self.grid.set_max_columns(cols);
        }
    }
}

pub fn attach_preview_on_hits(
    widget: &impl IsA<gtk::Widget>,
    selection: impl IsA<gtk::SelectionModel> + Clone,
    window: gtk::ApplicationWindow,
    preview_slot: Rc<RefCell<Option<gtk::Window>>>,
    hovered: Rc<RefCell<Option<String>>>,
    mode: Rc<Cell<PreviewMode>>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::space || key == gdk::Key::KP_Space {
            if let Some(path) = preview_path(mode.get(), &hovered, &selection) {
                preview(window.upcast_ref(), &path, &preview_slot);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    widget.add_controller(keys);
}

pub fn preview_path(
    mode: PreviewMode,
    hovered: &RefCell<Option<String>>,
    selection: &impl IsA<gtk::SelectionModel>,
) -> Option<String> {
    match mode {
        PreviewMode::Hovered => hovered
            .borrow()
            .clone()
            .or_else(|| selected_row(selection).map(|r| r.path())),
        PreviewMode::Selected => selected_row(selection)
            .map(|r| r.path())
            .or_else(|| hovered.borrow().clone()),
    }
}

pub fn attach_hover(
    row: &impl IsA<gtk::Widget>,
    item: gtk::ListItem,
    hovered: Rc<RefCell<Option<String>>>,
) {
    let motion = gtk::EventControllerMotion::new();
    {
        let item = item.clone();
        let hovered = Rc::clone(&hovered);
        motion.connect_enter(move |_, _, _| {
            if let Some(data) = item.item().and_downcast::<RowData>() {
                hovered.replace(Some(data.path()));
            }
        });
    }
    motion.connect_leave(move |_| {
        hovered.replace(None);
    });
    row.add_controller(motion);
}

pub fn attach_zoom_scroll(widget: &impl IsA<gtk::Widget>, host: Rc<Host>) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    let acc = Cell::new(0.0);
    scroll.connect_scroll(move |ctrl, _dx, dy| {
        if !ctrl
            .current_event_state()
            .contains(gdk::ModifierType::CONTROL_MASK)
        {
            return glib::Propagation::Proceed;
        }
        let a = acc.get() + dy;
        if a.abs() < 0.35 {
            acc.set(a);
            return glib::Propagation::Stop;
        }
        acc.set(0.0);
        let steps = if a < 0.0 { 1 } else { -1 };
        let next = host.zoom.get().bump(steps);
        if next != host.zoom.get() {
            host.zoom.set(next);
            host.schedule_apply();
        }
        glib::Propagation::Stop
    });
    widget.add_controller(scroll);
}

pub fn rebuild_tree(host: &Host, catalog: &Catalog, ids: &[u32]) {
    host.collapsed.borrow_mut().clear();
    *host.tree_src.borrow_mut() = Some((catalog.clone(), ids.to_vec()));
    fill_tree(host);
}

pub fn toggle_fold(host: &Host, path: &str) {
    {
        let mut collapsed = host.collapsed.borrow_mut();
        if !collapsed.remove(path) {
            collapsed.insert(path.to_string());
        }
    }
    fill_tree(host);
}

fn fill_tree(host: &Host) {
    let Some((catalog, ids)) = host.tree_src.borrow().clone() else {
        return;
    };
    let items: Vec<HitRef> = ids
        .iter()
        .filter_map(|&id| {
            let hit = catalog.hit(id)?;
            Some(HitRef {
                id: Some(id),
                path: hit.path().to_string_lossy().into_owned(),
                is_dir: hit.is_dir(),
                weight: hit.size().max(1),
            })
        })
        .collect();
    let stems = fold_stems(&items);
    let collapsed = host.collapsed.borrow();
    let flat = walk_visible(&stems, &|p| !collapsed.contains(p));
    drop(collapsed);
    host.tree_store.remove_all();
    for row in flat {
        host.tree_store.append(&RowData::with_fold(
            row.stem.name,
            row.stem.path,
            row.stem.is_dir,
            row.depth,
            row.has_kids,
        ));
    }
}

pub fn rebuild_weight(host: &Host, catalog: &Catalog, ids: &[u32], dir: Option<&std::path::Path>) {
    let items: Vec<HitRef> = ids
        .iter()
        .filter_map(|&id| {
            let hit = catalog.hit(id)?;
            let path = hit.path().to_string_lossy().into_owned();
            // Names-first hits store size 0: without a live stat every tile
            // weighs 1 and the chart lies about proportions.
            let mut size = hit.size();
            if size == 0 && !hit.is_dir() {
                size = std::fs::metadata(hit.path())
                    .map(|meta| meta.len())
                    .unwrap_or(0);
            }
            // Scope the chart to the browsed directory: tiles group by the
            // top level under it instead of scattering worldwide.
            let scoped = match dir {
                Some(root) => std::path::Path::new(&path)
                    .strip_prefix(root)
                    .ok()?
                    .to_string_lossy()
                    .into_owned(),
                None => path,
            };
            if scoped.is_empty() {
                return None;
            }
            Some(HitRef {
                id: Some(id),
                path: scoped,
                is_dir: hit.is_dir(),
                weight: size.max(1),
            })
        })
        .collect();
    rebuild_weight_values(host, folder_weights(&items));
}

pub fn rebuild_weight_values(host: &Host, weights: Vec<Weighted>) {
    *host.weights.borrow_mut() = weights;
    host.weight_rev.set(host.weight_rev.get().wrapping_add(1));
    host.weight.queue_draw();
}

/// Tiles below legibility are dropped: they cost layout + text per frame
/// while resizing and would render sub-pixel anyway.
const MAX_TILES: usize = 400;

pub fn make_weight_area(
    weights: Rc<RefCell<Vec<Weighted>>>,
    revision: Rc<Cell<u64>>,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(132);
    area.set_hexpand(true);
    let weights_draw = Rc::clone(&weights);
    // Two-level cache: the sorted/truncated items only change when the data
    // revision does; the tile layout only additionally needs the size.
    // Resize drags redraw every frame, so neither may re-sort the full set.
    struct WeightCache {
        w: i32,
        h: i32,
        rev: u64,
        items: Vec<Weighted>,
        tiles: Vec<Tile>,
    }
    let cache: Rc<RefCell<WeightCache>> = Rc::new(RefCell::new(WeightCache {
        w: 0,
        h: 0,
        rev: u64::MAX,
        items: Vec::new(),
        tiles: Vec::new(),
    }));
    let tooltip_cache = Rc::clone(&cache);
    area.set_has_tooltip(true);
    area.connect_query_tooltip(move |_, x, y, keyboard, tooltip| {
        if keyboard { return false; }
        let cache = tooltip_cache.borrow();
        let Some(tile) = cache.tiles.iter().find(|tile| f64::from(x) >= tile.x && f64::from(x) < tile.x + tile.w
            && f64::from(y) >= tile.y && f64::from(y) < tile.y + tile.h) else { return false; };
        tooltip.set_text(Some(&tile.path));
        true
    });
    area.set_draw_func(move |_, cr, w, h| {
        let rev = revision.get();
        let tiles = {
            let mut cache = cache.borrow_mut();
            if cache.rev != rev {
                let mut items = weights_draw.borrow().clone();
                items.sort_by(|a, b| b.weight.cmp(&a.weight));
                items.truncate(MAX_TILES);
                cache.items = items;
                cache.rev = rev;
                cache.w = -1;
            }
            if cache.w != w || cache.h != h || cache.tiles.is_empty() {
                cache.tiles = squarify(
                    cache.items.clone(),
                    f64::from(w.max(1)),
                    f64::from(h.max(1)),
                );
                cache.w = w;
                cache.h = h;
            }
            cache.tiles.clone()
        };
        // No background fill: the themed sidebar behind the area shows
        // through, so the chart always matches the panel around it.
        for (i, t) in tiles.iter().enumerate() {
            let (r, g, b) = tile_color(i, &t.path);
            cr.set_source_rgb(r, g, b);
            cr.rectangle(
                t.x + 1.0,
                t.y + 1.0,
                (t.w - 2.0).max(0.0),
                (t.h - 2.0).max(0.0),
            );
            let _ = cr.fill();
            // Labels only where they stay legible: cairo's toy text is the
            // most expensive call per frame during a resize drag.
            if t.w > 72.0 && t.h > 22.0 {
                let _ = cr.save();
                cr.rectangle(t.x + 4.0, t.y + 2.0, (t.w - 8.0).max(0.0), (t.h - 4.0).max(0.0));
                cr.clip();
                cr.set_source_rgb(0.98, 0.98, 1.0);
                cr.move_to(t.x + 6.0, t.y + 14.0);
                let _ = cr.show_text(&t.name);
                let _ = cr.restore();
            }
        }
    });
    area
}

pub(crate) fn tile_color(i: usize, path: &str) -> (f64, f64, f64) {
    let mut h = 216_613_6261u32;
    for b in path.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16777619);
    }
    h = h.wrapping_add(i as u32 * 17);
    let hue = (h % 360) as f64;
    hsl(hue, 0.42, 0.38)
}

fn hsl(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (r + m, g + m, b + m)
}

pub fn make_name_line(heading: bool) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("qfind-name");
    line.set_hexpand(true);
    line.set_halign(gtk::Align::Fill);
    let name = gtk::Label::new(None);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_single_line_mode(true);
    name.set_hexpand(true);
    if heading {
        name.add_css_class("heading");
    } else {
        name.add_css_class("caption");
    }
    line.append(&name);
    line
}

pub fn fill_name_line(line: &gtk::Box, name: &str, _is_dir: bool) {
    if let Some(label) = line.first_child().and_downcast::<gtk::Label>() {
        label.set_text(name);
    }
}

/// Hover feedback is independent of file selection and preview generation.
pub fn highlight_path(root: &gtk::Widget, path: Option<&std::path::Path>) {
    walk_apply(root, &|widget| {
        if !widget.has_css_class("qfind-item") { return; }
        let related = widget.tooltip_text().is_some_and(|item| {
            let item = std::path::Path::new(item.as_str());
            path.is_some_and(|path| path.starts_with(item) || item.starts_with(path))
        });
        if related { widget.add_css_class("qfind-chart-hover"); }
        else { widget.remove_css_class("qfind-chart-hover"); }
    });
}

fn restyle_list(list: &gtk::ColumnView, zoom: Zoom, spacing: u8) {
    walk_apply(list.upcast_ref(), &|w| {
        if w.has_css_class("qfind-row") {
            apply_list_metrics(w, zoom, spacing);
        }
    });
}

fn restyle_grid(grid: &gtk::GridView, zoom: Zoom) {
    walk_apply(grid.upcast_ref(), &|w| {
        if w.has_css_class("qfind-tile") {
            apply_grid_metrics(w, zoom);
        }
    });
}

fn walk_apply(w: &gtk::Widget, f: &impl Fn(&gtk::Widget)) {
    f(w);
    let mut child = w.first_child();
    while let Some(n) = child {
        walk_apply(&n, f);
        child = n.next_sibling();
    }
}

/// Name column of the details view: icon, name, dim path. Owns the row's
/// drag source, hover tracking, and right-click menu.
pub fn make_name_factory(
    selection: gtk::MultiSelection,
    popover: gtk::PopoverMenu,
    hovered: Rc<RefCell<Option<String>>>,
    zebra: Rc<Cell<bool>>,
    zoom: Rc<Cell<Zoom>>,
    spacing: Rc<Cell<u8>>,
    icons: Rc<RefCell<std::collections::HashMap<String, gio::Icon>>>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    {
        let selection_for_row = selection.clone();
        let popover_for_row = popover.clone();
        let hovered_setup = Rc::clone(&hovered);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.set_margin_start(6);
            row.set_margin_end(6);
            row.add_css_class("qfind-row");
            row.add_css_class("qfind-item");
            let icon = gtk::Image::from_icon_name("folder");
            icon.set_pixel_size(16);
            let name = make_name_line(false);
            if let Some(label) = name.first_child() {
                label.remove_css_class("caption");
            }
            row.append(&icon);
            row.append(&name);
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::COPY);
            drag.set_propagation_phase(gtk::PropagationPhase::Capture);
            let list_item = item.clone();
            let selection_for_drag = selection_for_row.clone();
            drag.connect_prepare(move |source, _, _| {
                if let Some(widget) = source.widget() {
                    source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&widget))), 8, 8);
                }
                source.set_state(gtk::EventSequenceState::Claimed);
                let item = list_item.downcast_ref::<gtk::ListItem>()?;
                let data = item.item().and_downcast::<RowData>()?;
                let rows = selected_rows(&selection_for_drag);
                if selection_for_drag.is_selected(item.position()) && rows.len() > 1 {
                    let paths = rows.into_iter().map(|row| row.path()).collect::<Vec<_>>();
                    content_for_paths(&paths)
                } else {
                    content_for_path(&data.path())
                }
            });
            row.add_controller(drag);
            attach_hover(&row, item.clone(), Rc::clone(&hovered_setup));
            let right = gtk::GestureClick::new();
            right.set_button(gdk::BUTTON_SECONDARY);
            let list_item = item.clone();
            let selection = selection_for_row.clone();
            let popover = popover_for_row.clone();
            let row_for_pop = row.clone();
            right.connect_pressed(move |_, _, x, y| {
                let Some(li) = list_item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                if !selection.is_selected(li.position()) {
                    selection.select_item(li.position(), true);
                }
                popup_at(&popover, &row_for_pop, x, y);
            });
            row.add_controller(right);
            item.set_child(Some(&row));
        });
    }
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<RowData>() else {
            return;
        };
        let Some(row) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(icon) = row.first_child().and_downcast::<gtk::Image>() else {
            return;
        };
        let Some(name) = icon.next_sibling().and_downcast::<gtk::Box>() else {
            return;
        };
        let z = zoom.get();
        apply_list_metrics(row.upcast_ref(), z, spacing.get());
        row.set_margin_start(6 + (data.depth() as i32) * 14);
        row.set_tooltip_text(Some(&data.path()));
        row.remove_css_class("qfind-chart-hover");
        // Names-first catalog hits carry no size/mtime: stat once, cache on
        // the row, so Size/Modified fill in instead of showing dashes.
        ensure_meta(&data);
        paint_icon(&icon, &data, &icons);
        fill_name_line(&name, &data.name(), data.is_dir());
        paint_zebra(row.upcast_ref(), zebra.get(), item.position());
    });
    factory
}

/// Catalog hits carry no size/mtime: stat once per row and cache it, so
/// every column bind (name/size/modified, whichever runs first) sees real
/// values. Dirs keep size 0 but still learn their mtime.
fn ensure_meta(data: &RowData) {
    if data.modified() > 0 {
        return;
    }
    let Ok(meta) = std::fs::metadata(data.path()) else {
        return;
    };
    let size = if data.is_dir() {
        data.size()
    } else {
        meta.len()
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    data.fill_metadata(size, modified);
}

fn detail_cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_hexpand(true);
    label.set_xalign(0.0);
    label.set_single_line_mode(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.add_css_class("dim-label");
    label.set_margin_end(6);
    label
}

fn paint_zebra(cell: &gtk::Widget, zebra: bool, position: u32) {
    if zebra && position % 2 == 1 {
        cell.add_css_class("qfind-odd");
    } else {
        cell.remove_css_class("qfind-odd");
    }
}

fn watch_size(item: &gtk::ListItem, label: &gtk::Label, storage: crate::storage::Pane) {
    label.set_tooltip_text(Some("Indexed or saved size; missing sizes update in the background."));
    let item = item.downgrade();
    let label = label.downgrade();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        let (Some(item), Some(label)) = (item.upgrade(), label.upgrade()) else { return gtk::glib::ControlFlow::Break; };
        if !label.is_mapped() { return gtk::glib::ControlFlow::Continue; }
        if let Some(data) = item.item().and_downcast::<RowData>() {
            let text = if data.is_dir() { storage.indexed_size_text(std::path::Path::new(&data.path())) }
                else { crate::actions::human_size(data.size()) };
            if label.text() != text { label.set_text(&text); }

        }
        gtk::glib::ControlFlow::Continue
    });
}

/// Size column of the details view.
pub fn make_size_factory(zebra: Rc<Cell<bool>>, storage: crate::storage::Pane) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let bind_storage = storage.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = detail_cell("");
        label.set_xalign(1.0);
        watch_size(item, &label, storage.clone());
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<RowData>() else {
            return;
        };
        let Some(label) = item.child().and_downcast::<gtk::Label>() else {
            return;
        };
        ensure_meta(&data);
        let text = if data.is_dir() {
            bind_storage.indexed_size_text(std::path::Path::new(&data.path()))
        } else {
            crate::actions::human_size(data.size())
        };
        label.set_text(&text);
        paint_zebra(label.upcast_ref(), zebra.get(), item.position());
    });
    factory
}

/// Modified column of the details view (relative age, em dash when unknown).
pub fn make_modified_factory(zebra: Rc<Cell<bool>>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_child(Some(&detail_cell("")));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<RowData>() else {
            return;
        };
        ensure_meta(&data);
        label_set_age(item, &data);
        if let Some(label) = item.child().and_downcast::<gtk::Label>() {
            paint_zebra(label.upcast_ref(), zebra.get(), item.position());
        }
    });
    factory
}

fn label_set_age(item: &gtk::ListItem, data: &RowData) {
    let Some(label) = item.child().and_downcast::<gtk::Label>() else {
        return;
    };
    let text = if data.modified() <= 0 {
        "—".to_owned()
    } else {
        crate::actions::human_mtime(data.modified())
    };
    label.set_text(&text);
}

pub fn apply_list_metrics(row: &gtk::Widget, zoom: Zoom, spacing: u8) {
    row.set_size_request(-1, 24 + i32::from(zoom.get().min(39)) / 6);
    let pad = 1 + i32::from(spacing) / 3;
    row.set_margin_top(pad);
    row.set_margin_bottom(pad);
    if let Some(icon) = row.first_child().and_downcast::<gtk::Image>() {
        icon.set_pixel_size(18 + i32::from(zoom.get().min(39)) / 6);
    }
}

fn apply_grid_metrics(tile: &gtk::Widget, zoom: Zoom) {
    let cell = grid_cell_px(zoom);
    tile.set_size_request(cell, cell);
    if let Some(media) = tile.first_child().and_downcast::<gtk::Stack>()
        && let Some(icon) = media.child_by_name("icon").and_downcast::<gtk::Image>()
    {
        icon.set_pixel_size((cell - 66).clamp(32, 96));
    }
}

fn grid_cell_px(zoom: Zoom) -> i32 {
    (zoom.cell_px() - 24).max(96)
}

pub fn popup_at(popover: &gtk::PopoverMenu, widget: &impl IsA<gtk::Widget>, x: f64, y: f64) {
    let widget = widget.as_ref();
    match popover.parent() {
        Some(parent) if &parent == widget => {}
        Some(_) => {
            popover.unparent();
            popover.set_parent(widget);
        }
        None => popover.set_parent(widget),
    }
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
}

pub fn make_grid_factory(
    selection: gtk::MultiSelection,
    popover: gtk::PopoverMenu,
    zebra: Rc<Cell<bool>>,
    zoom: Rc<Cell<Zoom>>,
    icons: Rc<RefCell<std::collections::HashMap<String, gio::Icon>>>,
    hovered: Rc<RefCell<Option<String>>>,
    storage: crate::storage::Pane,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    {
        let selection = selection.clone();
        let popover = popover.clone();
        let hovered = Rc::clone(&hovered);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
            col.add_css_class("qfind-tile");
            col.set_overflow(gtk::Overflow::Hidden);
            col.set_halign(gtk::Align::Fill);
            col.set_valign(gtk::Align::Fill);
            let icon = gtk::Image::from_icon_name("folder");
            icon.set_pixel_size(48);
            icon.set_halign(gtk::Align::Center);
            icon.set_valign(gtk::Align::Center);
            let picture = gtk::Picture::new();
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            let media = gtk::Stack::new();
            media.set_vexpand(true);
            media.add_named(&icon, Some("icon"));
            media.add_named(&picture, Some("picture"));
            let name = make_name_line(false);
            if let Some(label) = name.first_child().and_downcast::<gtk::Label>() {
                label.set_xalign(0.5);
                label.remove_css_class("caption");
            }
            name.set_margin_start(4);
            name.set_margin_end(4);
            name.set_margin_bottom(2);
            col.append(&media);
            let captions = gtk::Box::new(gtk::Orientation::Vertical, 2);
            captions.set_vexpand(false);
            captions.set_valign(gtk::Align::End);
            name.set_vexpand(false);
            name.set_valign(gtk::Align::Center);
            captions.append(&name);
            let size = detail_cell("");
            size.set_xalign(0.5);
            size.set_vexpand(false);
            size.set_valign(gtk::Align::Center);
            size.set_margin_start(4);
            size.add_css_class("caption");
            watch_size(item, &size, storage.clone());
            captions.append(&size);
            col.append(&captions);
            attach_hover(&col, item.clone(), Rc::clone(&hovered));
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::COPY);
            drag.set_propagation_phase(gtk::PropagationPhase::Capture);
            let list_item = item.clone();
            let selection_for_drag = selection.clone();
            drag.connect_prepare(move |source, _, _| {
                if let Some(widget) = source.widget() {
                    source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&widget))), 8, 8);
                }
                source.set_state(gtk::EventSequenceState::Claimed);
                let item = list_item.downcast_ref::<gtk::ListItem>()?;
                let data = item.item().and_downcast::<RowData>()?;
                let rows = selected_rows(&selection_for_drag);
                if selection_for_drag.is_selected(item.position()) && rows.len() > 1 {
                    let paths = rows.into_iter().map(|row| row.path()).collect::<Vec<_>>();
                    content_for_paths(&paths)
                } else {
                    content_for_path(&data.path())
                }
            });
            col.add_controller(drag);
            let right = gtk::GestureClick::new();
            right.set_button(gdk::BUTTON_SECONDARY);
            let list_item = item.clone();
            let selection = selection.clone();
            let popover = popover.clone();
            let col_for_pop = col.clone();
            right.connect_pressed(move |_, _, x, y| {
                if let Some(li) = list_item.downcast_ref::<gtk::ListItem>() {
                    if !selection.is_selected(li.position()) {
                        selection.select_item(li.position(), true);
                    }
                }
                popup_at(&popover, &col_for_pop, x, y);
            });
            col.add_controller(right);
            item.set_child(Some(&col));
        });
    }
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<RowData>() else {
            return;
        };
        let Some(col) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(media) = col.first_child().and_downcast::<gtk::Stack>() else {
            return;
        };
        let Some(icon) = media.child_by_name("icon").and_downcast::<gtk::Image>() else {
            return;
        };
        let Some(picture) = media
            .child_by_name("picture")
            .and_downcast::<gtk::Picture>()
        else {
            return;
        };
        let Some(name) = media.next_sibling().and_then(|captions| captions.first_child()).and_downcast::<gtk::Box>() else {
            return;
        };
        col.add_css_class("qfind-item");
        col.set_tooltip_text(Some(&data.path()));
        col.remove_css_class("qfind-chart-hover");
        apply_grid_metrics(col.upcast_ref(), zoom.get());
        paint_icon(&icon, &data, &icons);
        crate::actions::load_thumbnail(
            &media,
            &picture,
            std::path::Path::new(&data.path()),
            grid_cell_px(zoom.get()) as u32,
            grid_cell_px(zoom.get()) as u32,
        );
        fill_name_line(&name, &data.name(), data.is_dir());
        paint_zebra(col.upcast_ref(), zebra.get(), item.position());
    });
    factory
}

pub fn paint_icon(
    icon: &gtk::Image,
    data: &RowData,
    icons: &Rc<RefCell<std::collections::HashMap<String, gio::Icon>>>,
) {
    if data.is_dir() {
        // Keep one raster backing; zoom scales it instead of reloading an SVG per size.
        thread_local! {
            static FOLDER: Option<gdk::Texture> = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!("folder.png"))).ok();
        }
        FOLDER.with(|folder| icon.set_paintable(folder.as_ref()));
        return;
    }
    let key = data
        .name()
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let mut cache = icons.borrow_mut();
    let gicon = cache
        .entry(key)
        .or_insert_with(|| {
            let (ctype, _) =
                gio::content_type_guess(Some(std::path::Path::new(&data.name())), None::<&[u8]>);
            gio::content_type_get_icon(&ctype)
        })
        .clone();
    icon.set_from_gicon(&gicon);
}

pub fn make_tree_factory(
    toggle: Rc<RefCell<Box<dyn Fn(&str)>>>,
    collapsed: Rc<RefCell<HashSet<String>>>,
    icons: Rc<RefCell<std::collections::HashMap<String, gio::Icon>>>,
    hovered: Rc<RefCell<Option<String>>>,
    zebra: Rc<Cell<bool>>,
    storage: crate::storage::Pane,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    {
        let storage = storage.clone();
        let toggle = Rc::clone(&toggle);
        let hovered = Rc::clone(&hovered);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.set_margin_end(6);
            let twist = gtk::Button::new();
            twist.set_has_frame(false);
            twist.add_css_class("flat");
            twist.set_label("·");
            twist.set_valign(gtk::Align::Center);
            let list_item = item.clone();
            let toggle = Rc::clone(&toggle);
            twist.connect_clicked(move |_| {
                let Some(data) = list_item.item().and_downcast::<RowData>() else {
                    return;
                };
                if data.has_kids() {
                    (toggle.borrow())(&data.path());
                }
            });
            let icon = gtk::Image::from_icon_name("folder");
            icon.set_pixel_size(16);
            let name = make_name_line(true);
            name.set_hexpand(true);
            let size = detail_cell("");
            size.set_width_chars(10);
            watch_size(item, &size, storage.clone());
            let age = detail_cell("");
            age.set_width_chars(8);
            row.append(&twist);
            row.append(&icon);
            row.append(&name);
            row.append(&size);
            row.append(&age);
            attach_hover(&row, item.clone(), Rc::clone(&hovered));
            item.set_child(Some(&row));
        });
    }
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<RowData>() else {
            return;
        };
        let Some(row) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(twist) = row.first_child().and_downcast::<gtk::Button>() else {
            return;
        };
        let Some(icon) = twist.next_sibling().and_downcast::<gtk::Image>() else {
            return;
        };
        let Some(name) = icon.next_sibling().and_downcast::<gtk::Box>() else {
            return;
        };
        let Some(size) = name.next_sibling().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(age) = size.next_sibling().and_downcast::<gtk::Label>() else {
            return;
        };
        ensure_meta(&data);
        row.add_css_class("qfind-item");
        row.set_tooltip_text(Some(&data.path()));
        row.remove_css_class("qfind-chart-hover");
        row.set_margin_start(6 + data.depth() as i32 * 16);
        if data.has_kids() {
            let open = !collapsed.borrow().contains(&data.path());
            twist.set_label(if open { "▾" } else { "▸" });
            twist.set_sensitive(true);
        } else {
            twist.set_label(if data.is_dir() { "▸" } else { "·" });
            twist.set_sensitive(false);
        }
        paint_icon(&icon, &data, &icons);
        fill_name_line(&name, &data.name(), data.is_dir());
        let size_text = if data.is_dir() {
            storage.indexed_size_text(std::path::Path::new(&data.path()))
        } else {
            crate::actions::human_size(data.size())
        };
        size.set_text(&size_text);
        let age_text = if data.modified() <= 0 {
            "—".to_owned()
        } else {
            crate::actions::human_mtime(data.modified())
        };
        age.set_text(&age_text);
        paint_zebra(row.upcast_ref(), zebra.get(), item.position());
    });
    factory
}
