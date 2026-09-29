use std::cell::{Cell, RefCell};
use std::f64::consts::{PI, TAU};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use qfind_core::{Catalog, ChartScope, ManagerSession, StorageEntry, StorageMap};

use crate::glpie::{self, GlPie};
use crate::surface::tile_color;

/// Fraction of the view's smaller side used by the pie radius.
const RADIUS_FRAC: f64 = 0.47;
/// Fraction of the radius where the first ring starts.
const INNER_FRAC: f64 = 0.23;
/// Concentric drill-down rings.
const RING_COUNT: f64 = 4.0;

#[derive(Clone)]
struct Arc {
    entry: StorageEntry,
    start: f64,
    end: f64,
    inner: f64,
    outer: f64,
    color: (f64, f64, f64),
}

/// Pixel-space cache key. The epoch covers data, drill-down, and scope, so
/// only the allocation size varies on top of it.
#[derive(Clone, Copy, PartialEq)]
struct LayoutKey {
    epoch: u64,
    width: i32,
    height: i32,
}

/// Shared chart state. Geometry lives here so every UI path (navigation,
/// hover, resize, label paint) refreshes through one function.
#[derive(Clone)]
struct ChartState {
    map: Rc<RefCell<Option<Rc<StorageMap>>>>,
    catalog_map: Rc<RefCell<Option<Rc<StorageMap>>>>,
    current: Rc<Cell<Option<u32>>>,
    manager: Rc<RefCell<ManagerSession>>,
    /// Unit-space arcs (center 0,0, outer radius <= 1): recomputed only when
    /// the epoch changes, never on resize.
    unit: Rc<RefCell<Vec<Arc>>>,
    unit_epoch: Rc<Cell<u64>>,
    /// Whether the chart is weighted by bytes or by item count. Fixed by the
    /// user's choice and by what the data can support, never by accident.
    by_bytes: Rc<Cell<bool>>,
    /// The unit toggle pair, weakly held. A strong pair would make
    /// `button -> closure -> state -> button` a cycle, and the unit is also
    /// changed by a data set that carries no byte information at all, so the
    /// buttons have to be reachable from here.
    unit_buttons: Rc<RefCell<Vec<gtk::glib::WeakRef<gtk::ToggleButton>>>>,
    /// Heading of the WeightMap strip, which names the unit in use.
    map_title: gtk::Label,
    /// True while the browsed folder is being scanned. The centre total would
    /// otherwise keep rendering the previous folder's bytes.
    measuring: Rc<Cell<bool>>,
    /// The browsed folder, so the "Other" tile has a real path.
    chart_root: Rc<RefCell<Option<PathBuf>>>,
    /// Pixel-space arcs for hit-testing and labels: cheap rescale per size.
    arcs: Rc<RefCell<Vec<Arc>>>,
    pixel_key: Rc<RefCell<Option<LayoutKey>>>,
    layout_gen: Rc<Cell<u64>>,
    pie: GlPie,
    hover_index: Rc<Cell<Option<usize>>>,
    hover_layer: gtk::DrawingArea,
    hover: Rc<RefCell<Box<dyn Fn(Option<&StorageEntry>)>>>,
    hover_label: gtk::Label,
    weights: Rc<RefCell<Vec<qfind_core::Weighted>>>,
    weight_rev: Rc<Cell<u64>>,
    treemap: gtk::DrawingArea,
    capacity: Rc<Cell<Option<(u64, u64)>>>,
    live_nodes: Rc<RefCell<Option<Vec<StorageEntry>>>>,
    sizes: crate::folder_sizes::Sizes,
}

/// Recompute unit geometry on epoch change (map walk plus one GPU upload)
/// and rescale pixel arcs on size change (a few hundred multiplies). Never
/// measures item labels; hover feedback is a separate, lightweight overlay.
fn refresh_geometry(state: &ChartState, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let map_ref = state.map.borrow();
    let map = map_ref.as_deref();
    if map.is_none() && state.live_nodes.borrow().is_none() {
        return;
    }
    let epoch = state.layout_gen.get();
    if state.unit_epoch.get() != epoch {
        let manager = state.manager.borrow();
        let global = manager.chart_scope() == ChartScope::Global;
        // The chart's meaning has to be stable. It used to be "whichever the
        // data happens to have", so browsing a folder of empty files switched
        // the ring to counting entries and creating one non-empty file switched
        // it back, with the centre total still in bytes.
        let by_bytes = state.by_bytes.get();
        // `root` is the browsed folder; the "Other" tile is anchored to it so it
        // has a real path and therefore a tooltip.
        let root = state
            .chart_root
            .borrow()
            .clone()
            .unwrap_or_else(|| PathBuf::from("/"));
        let nodes = if !global && state.live_nodes.borrow().is_some() {
            let mut nodes = state.live_nodes.borrow().clone().unwrap_or_default();
            // Order and aggregate in the unit on screen. This branch sorted the
            // tiles and summed the "Other" wedge by bytes while the ring drew
            // every wedge by entry count, so the biggest wedge was whichever
            // file happened to be largest rather than the one on screen.
            let weight = |node: &StorageEntry| if by_bytes { node.bytes } else { node.entries };
            nodes.sort_by_key(|node| std::cmp::Reverse(weight(node)));
            let remaining = nodes
                .iter()
                .skip(63)
                .fold(0u64, |sum, node| sum.saturating_add(weight(node)));
            nodes.truncate(63);
            if remaining > 0 {
                // A real path, so the tile has a tooltip and can be described.
                // It used to be `PathBuf::new()`, which made the largest
                // aggregate wedge in the chart hover with an empty string.
                nodes.push(StorageEntry {
                    id: u32::MAX,
                    name: "Other".into(),
                    path: root.join("Other"),
                    is_dir: false,
                    bytes: if by_bytes { remaining } else { 0 },
                    entries: if by_bytes { 0 } else { remaining },
                });
            }
            nodes
        } else if global || state.current.get().is_some() {
            map.map(|map| visible_children(map, state.current.get(), by_bytes))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        *state.weights.borrow_mut() = nodes
            .iter()
            .map(|node| qfind_core::Weighted {
                name: node.name.clone(),
                path: node.path.to_string_lossy().into_owned(),
                weight: if by_bytes { node.bytes } else { node.entries },
                id: Some(node.id),
            })
            .collect();
        state.weight_rev.set(epoch);
        state.treemap.queue_draw();
        clear_hover(state);
        let mut unit = Vec::new();
        layout(
            map,
            nodes,
            -PI / 2.0,
            -PI / 2.0 + TAU,
            0,
            INNER_FRAC,
            (0.87 - INNER_FRAC) / RING_COUNT,
            by_bytes,
            &mut unit,
        );
        if !global && let Some((total, free)) = state.capacity.get().filter(|(total, _)| *total > 0)
        {
            let split = -PI / 2.0 + TAU * total.saturating_sub(free) as f64 / total as f64;
            for (name, bytes, start, end, color) in [
                (
                    "Used on volume",
                    total.saturating_sub(free),
                    -PI / 2.0,
                    split,
                    (0.32, 0.49, 0.70),
                ),
                (
                    "Free on volume",
                    free,
                    split,
                    -PI / 2.0 + TAU,
                    (0.30, 0.72, 0.59),
                ),
            ] {
                unit.push(Arc {
                    entry: StorageEntry {
                        id: u32::MAX,
                        name: name.into(),
                        path: PathBuf::new(),
                        is_dir: false,
                        bytes,
                        entries: 0,
                    },
                    start,
                    end,
                    inner: 0.93,
                    outer: 1.0,
                    color,
                });
            }
        }
        drop(manager);
        let slices: Vec<glpie::SliceGeom> = unit
            .iter()
            .map(|arc| glpie::SliceGeom {
                start: arc.start,
                end: arc.end,
                inner: arc.inner,
                outer: arc.outer,
                color: arc.color,
                id: arc.entry.id,
            })
            .collect();
        *state.unit.borrow_mut() = unit;
        state.unit_epoch.set(epoch);
        let (tris, lines) = glpie::tessellate(&slices);
        state.pie.set_geometry(tris, lines);
    }
    let key = LayoutKey {
        epoch,
        width,
        height,
    };
    if state.pixel_key.borrow().as_ref() != Some(&key) {
        let radius = f64::from(width.min(height)) * RADIUS_FRAC;
        let scaled: Vec<Arc> = state
            .unit
            .borrow()
            .iter()
            .map(|arc| Arc {
                entry: arc.entry.clone(),
                start: arc.start,
                end: arc.end,
                inner: arc.inner * radius,
                outer: arc.outer * radius,
                color: arc.color,
            })
            .collect();
        *state.arcs.borrow_mut() = scaled;
        *state.pixel_key.borrow_mut() = Some(key);
    }
}

fn clear_hover(state: &ChartState) {
    if state.hover_index.take().is_some() {
        state.hover_layer.queue_draw();
        state.hover.borrow()(None);
    }
    state.pie.view.set_cursor_from_name(None);
    state
        .hover_label
        .set_text("Hover to locate · click a folder to explore");
    state.hover_label.set_tooltip_text(None);
}

/// Record a unit choice and show it in the toggle pair.
///
/// Returns whether the unit actually changed, so the caller decides whether a
/// redraw is worth it. The toggle pair is updated from here rather than from the
/// button's own handler, because the standing unit is also the answer to
/// "what can this data set describe", not only to a button press.
fn set_unit(state: &ChartState, by_bytes: bool) -> bool {
    if state.by_bytes.get() == by_bytes {
        return false;
    }
    state.by_bytes.set(by_bytes);
    for (index, button) in state.unit_buttons.borrow().iter().enumerate() {
        let Some(button) = button.upgrade() else {
            continue;
        };
        // `unit_buttons` is ordered "Size", "Items".
        let wanted = (index == 0) == by_bytes;
        if button.is_active() != wanted {
            button.set_active(wanted);
        }
    }
    state.map_title.set_text(if by_bytes {
        "Space by size"
    } else {
        "Items by count"
    });
    state.layout_gen.set(state.layout_gen.get().wrapping_add(1));
    true
}

/// Decide the unit for a data set that has just arrived, once.
///
/// The unit used to be "whichever the data happens to have", recomputed on
/// every frame, so browsing a folder of empty files switched the ring to
/// counting entries and creating one non-empty file switched it back — with the
/// centre total still in bytes. Now the user's choice stands, and the only thing
/// that overrides it is a deliberate single fall back to counting entries when a
/// data set carries no byte information at all. The fall back is *written* into
/// `by_bytes`, so the next frame reads the same answer instead of re-deciding.
fn adopt_unit(state: &ChartState, has_bytes: bool) -> bool {
    if state.by_bytes.get() && !has_bytes {
        return set_unit(state, false);
    }
    false
}

#[derive(Clone)]
/// The pieces of a `Pane` a scope button needs, as plain `Rc`s.
///
/// The scope buttons live inside `Pane::root` and `Pane` holds a strong `root`, so
/// capturing the `Pane` in their handlers made `root -> button -> closure ->
/// Pane -> root` an uncollectable cycle. The window and its 1 Hz timer then
/// outlived every close.
struct Shared {
    state: ChartState,
    capacity_path: Rc<RefCell<PathBuf>>,
    detail: gtk::Label,
    title: gtk::Label,
}

impl Shared {
    fn of(pane: &Pane) -> Self {
        Self {
            state: pane.state.clone(),
            capacity_path: Rc::clone(&pane.capacity_path),
            detail: pane.detail.clone(),
            title: pane.title.clone(),
        }
    }

    fn set_directory_scope(&self, scope: ChartScope) {
        self.state.manager.borrow_mut().set_chart_scope(scope);
        match scope {
            ChartScope::Directory => {
                let directory = self
                    .state
                    .manager
                    .borrow()
                    .directory()
                    .map(Path::to_path_buf);
                if let Some(path) = directory {
                    Pane::set_directory_shared(
                        &self.state,
                        &self.capacity_path,
                        &self.detail,
                        &self.title,
                        &path,
                    );
                }
            }
            ChartScope::Global => {
                self.state.current.set(None);
                self.state.measuring.set(false);
                self.state.arcs.borrow_mut().clear();
                *self.state.pixel_key.borrow_mut() = None;
                *self.state.map.borrow_mut() = self.state.catalog_map.borrow().clone();
                let global_bytes = self
                    .state
                    .catalog_map
                    .borrow()
                    .as_ref()
                    .is_some_and(|map| map.total_bytes() > 0);
                refresh_labels(
                    &self.state.map.borrow(),
                    None,
                    true,
                    None,
                    &self.title,
                    &self.detail,
                );
                adopt_unit(&self.state, global_bytes);
                redraw_state(&self.state);
            }
        }
    }
}

#[derive(Clone)]
pub struct Pane {
    pub root: gtk::Box,
    title: gtk::Label,
    capacity_label: gtk::Label,
    capacity_path: Rc<RefCell<PathBuf>>,
    detail: gtk::Label,
    state: ChartState,
    catalog_generation: Rc<Cell<u64>>,
    projects: Rc<RefCell<Option<Vec<crate::manager_tools::Project>>>>,
    project_account: Rc<RefCell<Option<String>>>,
    project_generation: Rc<Cell<u64>>,
    /// Generation of the project refresh currently in flight, if any.
    project_running: Rc<Cell<Option<u64>>>,
    project_revision: Rc<Cell<u64>>,
    project_error: Rc<RefCell<Option<String>>>,
    sizes: crate::folder_sizes::Sizes,
    navigate: Rc<RefCell<Box<dyn Fn(PathBuf)>>>,
    select: Rc<RefCell<Box<dyn Fn(&StorageEntry) -> bool>>>,
    context: Rc<RefCell<Box<dyn Fn(&StorageEntry)>>>,
    context_menu: Rc<RefCell<Option<gtk::PopoverMenu>>>,
}

impl Pane {
    pub fn new(manager: Rc<RefCell<ManagerSession>>) -> Self {
        let title = gtk::Label::new(Some("Current directory"));
        title.add_css_class("title-3");
        title.set_xalign(0.5);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let detail = gtk::Label::new(Some("Opening Catalog…"));
        detail.add_css_class("dim-label");
        detail.set_xalign(0.5);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let pie = GlPie::new();
        pie.gl.set_widget_name("qfind-storage-map");
        let hover_layer = gtk::DrawingArea::new();
        hover_layer.set_can_target(false);
        hover_layer.set_hexpand(true);
        hover_layer.set_vexpand(true);
        pie.view.add_overlay(&hover_layer);
        let hover_label = gtk::Label::new(Some("Hover to locate · click a folder to explore"));
        hover_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        hover_label.set_lines(2);
        hover_label.set_justify(gtk::Justification::Center);
        let weights = Rc::new(RefCell::new(Vec::new()));
        let weight_rev = Rc::new(Cell::new(0));
        let treemap = crate::surface::make_weight_area(Rc::clone(&weights), Rc::clone(&weight_rev));
        treemap.set_content_height(150);

        let sizes = crate::folder_sizes::Sizes::new();
        let map_title = gtk::Label::new(Some("Space by size"));
        let state = ChartState {
            map: Rc::new(RefCell::new(None::<Rc<StorageMap>>)),
            catalog_map: Rc::new(RefCell::new(None::<Rc<StorageMap>>)),
            current: Rc::new(Cell::new(None)),
            manager,
            unit: Rc::new(RefCell::new(Vec::<Arc>::new())),
            unit_epoch: Rc::new(Cell::new(u64::MAX)),
            by_bytes: Rc::new(Cell::new(true)),
            unit_buttons: Rc::new(RefCell::new(Vec::new())),
            map_title: map_title.clone(),
            measuring: Rc::new(Cell::new(false)),
            chart_root: Rc::new(RefCell::new(None)),
            arcs: Rc::new(RefCell::new(Vec::<Arc>::new())),
            pixel_key: Rc::new(RefCell::new(None::<LayoutKey>)),
            layout_gen: Rc::new(Cell::new(0)),
            pie,
            hover_index: Rc::new(Cell::new(None)),
            hover_layer,
            hover_label,
            hover: Rc::new(RefCell::new(Box::new(|_| {}))),
            weights,
            weight_rev,
            treemap,
            capacity: Rc::new(Cell::new(None)),
            live_nodes: Rc::new(RefCell::new(None)),
            sizes: sizes.clone(),
        };
        let catalog_generation = Rc::new(Cell::new(0));
        let navigate: Rc<RefCell<Box<dyn Fn(PathBuf)>>> = Rc::new(RefCell::new(Box::new(|_| {})));
        let select: Rc<RefCell<Box<dyn Fn(&StorageEntry) -> bool>>> =
            Rc::new(RefCell::new(Box::new(|_| false)));
        let context: Rc<RefCell<Box<dyn Fn(&StorageEntry)>>> =
            Rc::new(RefCell::new(Box::new(|_| {})));
        let context_menu = Rc::new(RefCell::new(None::<gtk::PopoverMenu>));

        // Keep the pie legible: one total in the center, item details on hover.
        {
            let state = state.clone();
            let label_layer = state.pie.labels.clone();
            label_layer.set_draw_func(move |widget, cr, width, height| {
                refresh_geometry(&state, width, height);
                if state.map.borrow().is_none() && state.live_nodes.borrow().is_none() {
                    return;
                }
                let cx = f64::from(width) / 2.0;
                let cy = f64::from(height) / 2.0;
                let radius = f64::from(width.min(height)) * RADIUS_FRAC;
                let fg = widget.color();
                cr.set_source_rgba(
                    f64::from(fg.red()),
                    f64::from(fg.green()),
                    f64::from(fg.blue()),
                    0.9,
                );
                cr.set_font_size((radius * 0.09).clamp(12.0, 18.0));
                let by_bytes = state.by_bytes.get();
                let center = if state.measuring.get() {
                    // A scan is running for the folder just opened. Any total
                    // drawn here was the previous folder's, and the centre is
                    // the number people read as "this folder".
                    "Measuring…".into()
                } else if state.manager.borrow().chart_scope() != ChartScope::Global
                    && let Some(nodes) = state.live_nodes.borrow().as_ref()
                {
                    // The centre total has to speak the unit the wedges are
                    // drawn in. It used to stay in bytes while the ring counted
                    // entries, so the two never agreed.
                    let total = nodes.iter().fold(0u64, |sum, node| {
                        sum.saturating_add(if by_bytes { node.bytes } else { node.entries })
                    });
                    if by_bytes {
                        human_bytes(total)
                    } else {
                        format!("{total} items")
                    }
                } else if let Some(map) = state.map.borrow().as_ref() {
                    let node = state.current.get().and_then(|id| map.node(id));
                    if by_bytes && map.total_bytes() > 0 {
                        human_bytes(node.map_or(map.total_bytes(), |node| node.bytes))
                    } else {
                        format!(
                            "{} items",
                            node.map_or(map.total_entries(), |node| node.entries)
                        )
                    }
                } else {
                    String::new()
                };
                let extents = cr.text_extents(&center).ok();
                let x = extents.as_ref().map_or(cx, |e| cx - e.width() / 2.0);
                cr.move_to(x, cy + 5.0);
                let _ = cr.show_text(&center);
            });
        }

        {
            let state = state.clone();
            state
                .pie
                .gl
                .clone()
                .connect_resize(move |_, width, height| {
                    state.pie.set_view(
                        width as f32 / 2.0,
                        height as f32 / 2.0,
                        width.min(height) as f32 * RADIUS_FRAC as f32,
                    );
                    refresh_geometry(&state, state.pie.view.width(), state.pie.view.height());
                    state.pie.gl.queue_render();
                    state.pie.labels.queue_draw();
                    state.hover_layer.queue_draw();
                });
        }

        {
            let state = state.clone();
            let layer = state.hover_layer.clone();
            layer.set_draw_func(move |_, cr, width, height| {
                let arcs = state.arcs.borrow();
                let Some(arc) = state.hover_index.get().and_then(|index| arcs.get(index)) else {
                    return;
                };
                let (cx, cy) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
                cr.arc(cx, cy, arc.outer, arc.start, arc.end);
                cr.arc_negative(cx, cy, arc.inner, arc.end, arc.start);
                cr.close_path();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
            });
        }
        let motion = gtk::EventControllerMotion::new();
        {
            let state = state.clone();
            motion.connect_motion(move |_, x, y| {
                let view = &state.pie.view;
                refresh_geometry(&state, view.width(), view.height());
                let arcs = state.arcs.borrow();
                let next = hit(&arcs, view.width(), view.height(), x, y).and_then(|arc| {
                    arcs.iter()
                        .position(|candidate| std::ptr::eq(candidate, arc))
                });
                if next == state.hover_index.get() {
                    return;
                }
                state.hover_index.set(next);
                state.hover_layer.queue_draw();
                view.set_cursor_from_name(next.map(|_| "pointer"));
                let entry = next.map(|index| &arcs[index].entry);
                if let Some(entry) = entry {
                    state.hover_label.set_text(&format!(
                        "{} · {}",
                        entry.name,
                        human_bytes(entry.bytes)
                    ));
                    state
                        .hover_label
                        .set_tooltip_text(Some(&entry.path.to_string_lossy()));
                } else {
                    state
                        .hover_label
                        .set_text("Hover to locate · click a folder to explore");
                    state.hover_label.set_tooltip_text(None);
                }
                state.hover.borrow()(entry.filter(|entry| entry.id != u32::MAX));
            });
        }
        {
            let state = state.clone();
            motion.connect_leave(move |_| clear_hover(&state));
        }
        state.pie.view.add_controller(motion);
        {
            let state = state.clone();
            state
                .pie
                .view
                .clone()
                .connect_unmap(move |_| clear_hover(&state));
        }

        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        {
            let view = state.pie.view.clone();
            let state = state.clone();
            let navigate = Rc::clone(&navigate);
            click.connect_released(move |_, _, x, y| {
                refresh_geometry(&state, view.width(), view.height());
                let width = view.width();
                let height = view.height();
                let cx = f64::from(width) / 2.0;
                let cy = f64::from(height) / 2.0;
                let inner = f64::from(width.min(height)) * RADIUS_FRAC * INNER_FRAC;
                if (x - cx).hypot(y - cy) < inner {
                    let parent = if state.manager.borrow().chart_scope() == ChartScope::Directory {
                        state
                            .manager
                            .borrow()
                            .directory()
                            .and_then(Path::parent)
                            .map(Path::to_path_buf)
                    } else {
                        state
                            .map
                            .borrow()
                            .as_ref()
                            .and_then(|map| state.current.get().and_then(|id| map.node(id)))
                            .and_then(|node| node.path.parent().map(Path::to_path_buf))
                    };
                    if let Some(parent) = parent {
                        navigate.borrow()(parent);
                    }
                    return;
                }
                let next = hit(&state.arcs.borrow(), width, height, x, y)
                    .filter(|arc| arc.entry.is_dir)
                    .map(|arc| arc.entry.path.clone());
                if let Some(path) = next {
                    navigate.borrow()(path);
                }
            });
        }
        state.pie.view.add_controller(click);

        let right = gtk::GestureClick::new();
        right.set_button(gtk::gdk::BUTTON_SECONDARY);
        {
            let view = state.pie.view.clone();
            let state = state.clone();
            let select = Rc::clone(&select);
            let context = Rc::clone(&context);
            let context_menu = Rc::clone(&context_menu);
            right.connect_pressed(move |_, _, x, y| {
                refresh_geometry(&state, view.width(), view.height());
                let Some(entry) = hit(&state.arcs.borrow(), view.width(), view.height(), x, y)
                    .map(|arc| arc.entry.clone())
                    .filter(|entry| entry.id != u32::MAX)
                else {
                    return;
                };
                select.borrow()(&entry);
                context.borrow()(&entry);
                if let Some(menu) = context_menu.borrow().as_ref() {
                    crate::surface::popup_at(menu, &view, x, y);
                }
            });
        }
        state.pie.view.add_controller(right);

        let directory_btn = gtk::ToggleButton::with_label("Directory");
        directory_btn.set_active(true);
        let global_btn = gtk::ToggleButton::with_label("Global");
        global_btn.set_group(Some(&directory_btn));
        let scope = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        scope.add_css_class("linked");
        scope.set_halign(gtk::Align::Center);
        scope.append(&directory_btn);
        scope.append(&global_btn);
        // The WeightMap unit. This pair is what `by_bytes` is *for*: without it
        // the field was initialised once and could never be written, so the
        // chart was stuck on bytes no matter what the data supported.
        let size_btn = gtk::ToggleButton::with_label("Size");
        size_btn.set_active(true);
        size_btn.set_tooltip_text(Some("Weight the chart and the map by bytes"));
        let items_btn = gtk::ToggleButton::with_label("Items");
        items_btn.set_group(Some(&size_btn));
        items_btn.set_tooltip_text(Some("Weight the chart and the map by item count"));
        let unit = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        unit.add_css_class("linked");
        unit.set_halign(gtk::Align::Center);
        unit.set_margin_top(4);
        unit.append(&size_btn);
        unit.append(&items_btn);
        state
            .unit_buttons
            .borrow_mut()
            .extend([size_btn.downgrade(), items_btn.downgrade()]);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_margin_top(8);
        root.set_margin_bottom(8);
        root.set_margin_start(12);
        root.set_margin_end(12);
        root.append(&scope);
        // Under the scope buttons, because both rows describe the whole chart.
        root.append(&unit);
        root.append(&title);
        root.append(&detail);
        state.pie.root.set_size_request(-1, 220);
        root.append(&state.pie.root);
        let capacity_label = gtk::Label::new(Some("Volume capacity…"));
        capacity_label.add_css_class("caption");
        capacity_label.set_wrap(true);
        capacity_label.set_justify(gtk::Justification::Center);
        root.append(&capacity_label);
        root.append(&state.hover_label);
        map_title.add_css_class("heading");
        map_title.set_margin_top(8);
        root.append(&map_title);
        root.append(&state.treemap);
        let pane = Self {
            root,
            capacity_label,
            capacity_path: Rc::new(RefCell::new(PathBuf::new())),
            title,
            detail,
            state,
            catalog_generation,
            projects: Rc::new(RefCell::new(None)),
            project_account: Rc::new(RefCell::new(None)),
            project_generation: Rc::new(Cell::new(0)),
            project_running: Rc::new(Cell::new(None)),
            project_revision: Rc::new(Cell::new(0)),
            project_error: Rc::new(RefCell::new(None)),
            sizes,
            navigate,
            select,
            context,
            context_menu,
        };
        {
            // `Pane` owns `root`, and this button lives inside `root`, so
            // capturing it strongly makes `root -> button -> closure -> Pane ->
            // root` an uncollectable cycle: the window and its 1 Hz timer
            // outlive every close.
            let shared = Shared::of(&pane);
            directory_btn.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                shared.set_directory_scope(ChartScope::Directory);
            });
        }
        {
            let shared = Shared::of(&pane);
            global_btn.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                shared.set_directory_scope(ChartScope::Global);
            });
        }
        {
            // Only the `ChartState` is captured, never the `Pane`: these buttons
            // live in `Pane::root`, and a strong `Pane` here would be the same
            // `root -> button -> closure -> Pane -> root` cycle the scope
            // buttons above avoid.
            let state = pane.state.clone();
            size_btn.connect_toggled(move |button| {
                if button.is_active() && set_unit(&state, true) {
                    redraw_state(&state);
                }
            });
        }
        {
            let state = pane.state.clone();
            items_btn.connect_toggled(move |button| {
                if button.is_active() && set_unit(&state, false) {
                    redraw_state(&state);
                }
            });
        }
        {
            let root = pane.root.downgrade();
            let state = pane.state.clone();
            let detail = pane.detail.downgrade();
            let revision = Cell::new(0);
            gtk::glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
                let Some(root) = root.upgrade() else {
                    return gtk::glib::ControlFlow::Break;
                };
                if !root.is_mapped() || state.manager.borrow().chart_scope() == ChartScope::Global {
                    return gtk::glib::ControlFlow::Continue;
                }
                let current = state.sizes.revision();
                if revision.replace(current) == current {
                    return gtk::glib::ControlFlow::Continue;
                }
                // Only the bytes unit has anything to say about folder sizes,
                // and a measurement is queued once per navigation now (see
                // `set_directory_shared`), so this poll only *reads* what the
                // worker already measured. It used to re-queue a recursive
                // `du` for every child of the folder on screen on every
                // revision — including revisions caused by other windows — which
                // is what kept the 128-deep queue permanently saturated.
                if !state.by_bytes.get() || state.measuring.get() {
                    return gtk::glib::ControlFlow::Continue;
                }
                let mut changed = false;
                if let Some(nodes) = state.live_nodes.borrow_mut().as_mut() {
                    for node in nodes.iter_mut().filter(|node| node.is_dir) {
                        if let Some(bytes) = state.sizes.get(&node.path)
                            && bytes != node.bytes
                        {
                            node.bytes = bytes;
                            changed = true;
                        }
                    }
                }
                if changed {
                    state.layout_gen.set(state.layout_gen.get().wrapping_add(1));
                    refresh_geometry(&state, state.pie.view.width(), state.pie.view.height());
                    state.pie.gl.queue_render();
                    state.pie.labels.queue_draw();
                    if let Some(detail) = detail.upgrade() {
                        detail.set_text("Measured and indexed sizes · click center to go up");
                    }
                }
                gtk::glib::ControlFlow::Continue
            });
        }
        pane
    }

    pub fn indexed_size_text(&self, path: &Path) -> String {
        let pending = self.sizes.text(path);
        self.known_size(path)
            .map(crate::actions::human_size)
            .unwrap_or(pending)
    }

    pub fn known_size(&self, path: &Path) -> Option<u64> {
        self.sizes.get(path).or_else(|| self.indexed_size(path))
    }

    pub fn indexed_size(&self, path: &Path) -> Option<u64> {
        self.state
            .catalog_map
            .borrow()
            .as_ref()?
            .find_indexed(path)
            .map(|entry| entry.bytes)
    }

    pub fn catalog_revision(&self) -> u64 {
        self.project_revision.get()
    }

    pub fn project_error(&self) -> Option<String> {
        self.project_error.borrow().clone()
    }

    /// Forget a project error so the next attempt can be made.
    ///
    /// `refresh_projects` only cleared the error on the path that reached the
    /// index, so one transient failure left the Projects page reading
    /// "Projects are unavailable" for the rest of the session with no way back.
    pub fn clear_project_error(&self) {
        *self.project_error.borrow_mut() = None;
    }

    /// True while a project refresh is in flight.
    ///
    /// The generation is stamped when the work starts and cleared when it lands
    /// (or is superseded), so a UI can grey out a button for exactly as long as
    /// the work takes instead of guessing — and always get it back.
    pub fn project_refresh_pending(&self) -> bool {
        self.project_running.get().is_some()
    }

    pub fn projects(&self, root: &Path) -> Option<Vec<crate::manager_tools::Project>> {
        Some(
            self.projects
                .borrow()
                .as_ref()?
                .iter()
                .filter(|project| project.path.starts_with(root))
                .cloned()
                .collect(),
        )
    }

    pub fn set_hover(&self, hover: impl Fn(Option<&StorageEntry>) + 'static) {
        *self.state.hover.borrow_mut() = Box::new(hover);
    }

    pub fn set_navigate(&self, navigate: impl Fn(PathBuf) + 'static) {
        *self.navigate.borrow_mut() = Box::new(navigate);
    }

    pub fn bind_results(
        &self,
        menu: gtk::PopoverMenu,
        select: impl Fn(&StorageEntry) -> bool + 'static,
        context: impl Fn(&StorageEntry) + 'static,
    ) {
        *self.context_menu.borrow_mut() = Some(menu);
        *self.select.borrow_mut() = Box::new(select);
        *self.context.borrow_mut() = Box::new(context);
    }

    fn bump_layout(&self) {
        self.state
            .layout_gen
            .set(self.state.layout_gen.get().wrapping_add(1));
    }

    fn redraw(&self) {
        redraw_state(&self.state);
    }

    pub fn set_directory(&self, path: &Path) {
        Self::set_directory_shared(
            &self.state,
            &self.capacity_path,
            &self.detail,
            &self.title,
            path,
        );
        self.state.capacity.set(None);
        // The volume line belonged to the folder just left until the new
        // Mount's capacity landed, and on another volume it is a different
        // number entirely.
        self.capacity_label.set_text("Volume capacity…");
        let capacity_label = self.capacity_label.clone();
        let state = self.state.clone();
        let capacity_path = self.capacity_path.clone();
        let path = path.to_path_buf();
        gtk::glib::MainContext::default().spawn_local(async move {
            let result = gtk::gio::File::for_path(&path)
                .query_filesystem_info_future(
                    "filesystem::size,filesystem::free",
                    gtk::glib::Priority::DEFAULT,
                )
                .await;
            if *capacity_path.borrow() != path {
                return;
            }
            match result {
                Ok(info)
                    if info.has_attribute("filesystem::size")
                        && info.has_attribute("filesystem::free") =>
                {
                    let total = info.attribute_uint64("filesystem::size");
                    let free = info.attribute_uint64("filesystem::free").min(total);
                    state.capacity.set(Some((total, free)));
                    capacity_label.set_text(&format!(
                        "{} free of {} - volume",
                        human_bytes(free),
                        human_bytes(total)
                    ));
                }
                _ => state.capacity.set(None),
            }
        });
    }

    /// The half of [`Self::set_directory`] that a scope button can call without
    /// holding the `Pane`, and so without a `root -> button -> closure -> Pane ->
    /// root` reference cycle.
    fn set_directory_shared(
        state: &ChartState,
        capacity_path: &Rc<RefCell<PathBuf>>,
        detail: &gtk::Label,
        title: &gtk::Label,
        path: &Path,
    ) {
        *capacity_path.borrow_mut() = path.to_path_buf();
        *state.live_nodes.borrow_mut() = None;
        *state.chart_root.borrow_mut() = Some(path.to_path_buf());
        // Drill into the indexed child of this folder, when there is one, so
        // the indexed view is in place before the measuring state goes on.
        if state.manager.borrow().chart_scope() != ChartScope::Global
            && let Some(map) = state.catalog_map.borrow().clone()
        {
            let next = map.find(path).map(|node| node.id);
            *state.map.borrow_mut() = Some(map);
            state.current.set(next);
            state.layout_gen.set(state.layout_gen.get().wrapping_add(1));
            refresh_labels(&state.map.borrow(), next, false, Some(path), title, detail);
        }
        // Mark the chart as measuring *after* the drill-in: `refresh_labels` has
        // just written this folder's indexed numbers into the title and detail,
        // and a slow or network Mount must not keep them on screen next to a
        // chart that is still the previous folder's.
        begin_measuring(state, title, detail, path);
        let scan_state = state.clone();
        let scan_path = Rc::clone(capacity_path);
        let scan_detail = detail.clone();
        let directory = path.to_path_buf();
        gtk::glib::MainContext::default().spawn_local(async move {
            let scanned = directory.clone();
            let result = gtk::gio::spawn_blocking(move || read_children(&scanned)).await;
            // A newer navigation owns the chart now; leave its state alone.
            if *scan_path.borrow() != directory {
                return;
            }
            scan_state.measuring.set(false);
            let Ok(Ok(mut nodes)) = result else {
                // Leaving `measuring` set would keep the ring blank with
                // "Measuring …" under a folder that can never be measured.
                scan_detail.set_text("Could not read this folder");
                scan_state.pie.gl.queue_render();
                scan_state.pie.labels.queue_draw();
                return;
            };
            let map = scan_state.catalog_map.borrow();
            let mut unmeasured = Vec::new();
            for (index, node) in nodes.iter_mut().enumerate() {
                node.id = u32::MAX.saturating_sub(index as u32 + 1);
                if !node.is_dir {
                    continue;
                }
                // The Catalog's own index answers first, and it is already in
                // memory; the folder-size cache answers second. Only what is
                // still unknown is worth a measurement.
                if let Some(indexed) = map.as_ref().and_then(|map| map.find_indexed(&node.path)) {
                    node.id = indexed.id;
                    node.bytes = indexed.bytes;
                }
                if let Some(bytes) = scan_state.sizes.get(&node.path) {
                    node.bytes = bytes;
                }
                if node.bytes == 0 {
                    unmeasured.push(node.path.clone());
                }
            }
            drop(map);
            // One request pass per navigation, for the folder on screen, and
            // only while the chart is actually counting bytes. The listing
            // above used to queue a recursive `du` for *every* child of *every*
            // folder visited (twice: once through `components::storage_children`
            // and again through `Sizes::text`), and the 1 Hz poll kept
            // re-queueing them; the 128-deep queue then saturated and dropped
            // the work, while every completed job rewrote the whole JSON cache.
            if scan_state.by_bytes.get() {
                for path in unmeasured {
                    scan_state.sizes.request(&path);
                }
            }
            let has_bytes = nodes.iter().any(|node| node.bytes > 0);
            *scan_state.live_nodes.borrow_mut() = Some(nodes);
            scan_state
                .layout_gen
                .set(scan_state.layout_gen.get().wrapping_add(1));
            adopt_unit(&scan_state, has_bytes);
            redraw_state(&scan_state);
            scan_detail.set_text("Measured and indexed sizes - click center to go up");
        });
    }

    pub fn refresh_projects(&self, catalog: Catalog, force: bool) {
        // The cheap in-memory test first. `Catalog` adoption happens on every
        // navigation and subtree refresh, and it used to resolve the GitHub
        // account *first* — a `gh config get user` subprocess with a 15 s
        // timeout — before discovering there was nothing to do. Nothing is
        // spawned unless the work will actually be done.
        if !force && self.projects.borrow().is_some() {
            return;
        }
        let generation = self.project_generation.get().wrapping_add(1);
        self.project_generation.set(generation);
        self.project_running.set(Some(generation));
        let this = self.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            // Offline-first: GitHub account is best-effort enrichment only.
            // Local repositories must list even without `gh auth`.
            let account = gtk::gio::spawn_blocking(crate::manager_tools::active_project_account)
                .await
                .ok()
                .and_then(|result| result.ok())
                .unwrap_or_default();
            if this.project_generation.get() != generation {
                return;
            }
            let changed = this.project_account.borrow().as_ref() != Some(&account);
            if !changed && !force && this.projects.borrow().is_some() {
                this.project_running.set(None);
                return;
            }
            if changed {
                *this.projects.borrow_mut() = None;
                *this.project_account.borrow_mut() = Some(account.clone());
                this.project_revision
                    .set(this.project_revision.get().wrapping_add(1));
            }
            *this.project_error.borrow_mut() = None;
            let result = gtk::gio::spawn_blocking(move || {
                let projects = crate::manager_tools::index_projects(&catalog)?;
                Ok::<_, String>(projects)
            })
            .await;
            if this.project_generation.get() != generation {
                return;
            }
            match result {
                Ok(Ok(projects)) => *this.projects.borrow_mut() = Some(projects),
                Ok(Err(error)) => *this.project_error.borrow_mut() = Some(error),
                Err(_) => {
                    *this.project_error.borrow_mut() =
                        Some("Project indexing failed. Refresh to retry.".into())
                }
            }
            this.project_running.set(None);
            this.project_revision
                .set(this.project_revision.get().wrapping_add(1));
        });
    }

    pub fn set_catalog(&self, catalog: Catalog) {
        self.detail.set_text("Building chart…");
        // A subtree refresh rewrites the snapshot; that must not re-scan every repository.
        self.refresh_projects(catalog.clone(), false);
        let generation = self.catalog_generation.get().wrapping_add(1);
        self.catalog_generation.set(generation);
        let this = self.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            let result = gtk::gio::spawn_blocking(move || catalog.storage_map()).await;
            if this.catalog_generation.get() != generation {
                return;
            }
            let Ok(map) = result else {
                this.detail.set_text("Chart failed");
                return;
            };
            let map = Rc::new(map);
            let has_bytes = map.total_bytes() > 0;
            let manager = this.state.manager.borrow();
            let global = manager.chart_scope() == ChartScope::Global;
            let directory = manager.directory().map(Path::to_path_buf);
            let next = (!global)
                .then(|| {
                    directory
                        .as_deref()
                        .and_then(|path| map.find(path))
                        .map(|node| node.id)
                })
                .flatten();
            drop(manager);
            this.state.current.set(next);

            this.state.arcs.borrow_mut().clear();
            *this.state.catalog_map.borrow_mut() = Some(Rc::clone(&map));
            *this.state.map.borrow_mut() = Some(map);
            refresh_labels(
                &this.state.map.borrow(),
                next,
                global,
                directory.as_deref(),
                &this.title,
                &this.detail,
            );
            if this.state.measuring.get() {
                // A scan for this folder is still running, and the snapshot it
                // will be compared against predates the folder the user just
                // opened, so the measuring line wins over any numbers this
                // refresh can offer.
                this.detail.set_text("Measuring this folder…");
            } else if !global && this.state.live_nodes.borrow().is_some() {
                this.detail
                    .set_text("Measured and indexed sizes · click center to go up");
            }
            // A fresh index is a fresh data set: one deliberate unit decision
            // for it, never one per frame.
            adopt_unit(&this.state, has_bytes);
            this.bump_layout();
            this.redraw();
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn layout(
    map: Option<&StorageMap>,
    nodes: Vec<StorageEntry>,
    start: f64,
    end: f64,
    depth: usize,
    inner: f64,
    ring: f64,
    by_bytes: bool,
    out: &mut Vec<Arc>,
) {
    if depth == 4 || nodes.is_empty() || end - start < 0.002 {
        return;
    }
    let weight = |node: &StorageEntry| {
        if by_bytes {
            node.bytes.max(1)
        } else {
            node.entries.max(1)
        }
    };
    let total = nodes.iter().map(&weight).sum::<u64>().max(1) as f64;
    let mut angle = start;
    for (index, node) in nodes.into_iter().enumerate() {
        let next = angle + (end - start) * weight(&node) as f64 / total;
        let path = node.path.to_string_lossy();
        // Subpixel wedges are not useful at normal window sizes; drill in to reveal them.
        if next - angle < 0.002 {
            angle = next;
            continue;
        }
        let indexed_map = map.filter(|map| {
            node.is_dir
                && map.node(node.id).is_some_and(|indexed| {
                    indexed.path == node.path && (!by_bytes || indexed.bytes == node.bytes)
                })
                && map.has_children(node.id)
        });
        out.push(Arc {
            entry: node.clone(),
            start: angle,
            end: next,
            inner: inner + ring * depth as f64,
            outer: if indexed_map.is_some() {
                inner + ring * (depth + 1) as f64
            } else {
                inner + ring * RING_COUNT
            },
            color: tile_color(index + depth * 67, &path),
        });
        if let Some(indexed_map) = indexed_map {
            layout(
                map,
                visible_children(indexed_map, Some(node.id), by_bytes),
                angle,
                next,
                depth + 1,
                inner,
                ring,
                by_bytes,
                out,
            );
        }
        angle = next;
    }
}

fn visible_children(map: &StorageMap, parent: Option<u32>, by_bytes: bool) -> Vec<StorageEntry> {
    let mut nodes = map.children_limited(parent, 63);
    let (total_bytes, total_entries, path) = parent.and_then(|id| map.node(id)).map_or_else(
        || {
            (
                map.total_bytes(),
                map.total_entries(),
                "Other small items".into(),
            )
        },
        |node| {
            (
                node.bytes,
                node.entries,
                node.path.join("Other small items"),
            )
        },
    );
    let shown_bytes = nodes
        .iter()
        .fold(0u64, |total, node| total.saturating_add(node.bytes));
    let shown_entries = nodes
        .iter()
        .fold(0u64, |total, node| total.saturating_add(node.entries));
    let hidden_bytes = total_bytes.saturating_sub(shown_bytes);
    let hidden_entries = total_entries.saturating_sub(shown_entries);
    if (by_bytes && hidden_bytes > 0) || (!by_bytes && hidden_entries > 0) {
        nodes.push(StorageEntry {
            id: u32::MAX,
            name: "Other".into(),
            path,
            is_dir: false,
            bytes: hidden_bytes,
            entries: hidden_entries,
        });
    }
    nodes
}

fn hit(arcs: &[Arc], width: i32, height: i32, x: f64, y: f64) -> Option<&Arc> {
    let dx = x - f64::from(width) / 2.0;
    let dy = y - f64::from(height) / 2.0;
    let radius = dx.hypot(dy);
    let mut angle = dy.atan2(dx);
    if angle < -PI / 2.0 {
        angle += TAU;
    }
    arcs.iter().rev().find(|arc| {
        radius >= arc.inner && radius <= arc.outer && angle >= arc.start && angle <= arc.end
    })
}

fn refresh_labels(
    map: &Option<Rc<StorageMap>>,
    current: Option<u32>,
    global: bool,
    directory: Option<&Path>,
    title: &gtk::Label,
    detail: &gtk::Label,
) {
    let Some(map) = map else { return };
    if let Some(node) = current.and_then(|id| map.node(id)) {
        title.set_text(&node.name);
        title.set_tooltip_text(Some(&node.path.to_string_lossy()));
        let measure = if map.total_bytes() > 0 {
            format!("{} apparent size", human_bytes(node.bytes))
        } else {
            format!("{} indexed items", node.entries)
        };
        detail.set_text(&format!("{measure} · click center to go up"));
    } else if global {
        title.set_text("All indexed locations");
        let measure = if map.total_bytes() > 0 {
            format!("{} apparent size", human_bytes(map.total_bytes()))
        } else {
            format!("{} items · refresh Catalog for sizes", map.total_entries())
        };
        detail.set_text(&format!(
            "{measure} · {} indexed locations",
            map.roots().len()
        ));
    } else {
        title.set_text(
            &directory
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Current directory".into()),
        );
        detail.set_text("Not indexed · refresh the Catalog once for instant Chart navigation");
    }
}

fn human_bytes(bytes: u64) -> String {
    crate::actions::human_size(bytes)
}

/// Recompute the chart and ask for the two repaints that show it.
///
/// A free function so a button handler can repaint without holding the `Pane`
/// (and so without the `root -> button -> closure -> Pane -> root` cycle).
fn redraw_state(state: &ChartState) {
    let (width, height) = (state.pie.view.width(), state.pie.view.height());
    refresh_geometry(state, width, height);
    state.pie.gl.queue_render();
    state.pie.labels.queue_draw();
}

/// Put the chart into its "measuring this folder" state.
///
/// Everything on screen belongs to the folder the user just left until the scan
/// lands: the ring is still its wedges, the WeightMap strip its rectangles, the
/// centre its total, and the hover highlight its row. Reading those as the new
/// folder's numbers is the whole bug, so they are dropped here rather than
/// refreshed later. The epoch bump is what stops `refresh_geometry` from
/// rebuilding the same arcs out of the cached unit geometry.
fn begin_measuring(state: &ChartState, title: &gtk::Label, detail: &gtk::Label, path: &Path) {
    state.measuring.set(true);
    state.hover_index.set(None);
    state.arcs.borrow_mut().clear();
    *state.pixel_key.borrow_mut() = None;
    clear_hover(state);
    // The strip caches by revision, so an empty list plus a new revision is
    // what empties it.
    *state.weights.borrow_mut() = Vec::new();
    state.weight_rev.set(state.weight_rev.get().wrapping_add(1));
    state.treemap.queue_draw();
    state.layout_gen.set(state.layout_gen.get().wrapping_add(1));
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    title.set_text(&name);
    title.set_tooltip_text(Some(&path.display().to_string()));
    detail.set_text(&format!("Measuring {name}…"));
    redraw_state(state);
}

/// The immediate children of one folder, for the chart.
///
/// Not `qfind_core::components::storage_children`: that helper queues a
/// recursive `du` for every child folder it sees, so every navigation filled
/// the 128-deep measurement queue with work for folders nobody was looking at
/// and the tail was dropped when it saturated. This lists the directory and
/// nothing else; sizes that are already known are merged in by the caller, and
/// measurement is requested once per navigation for what is left. Called on a
/// worker thread, never on the interface thread.
fn read_children(path: &Path) -> Result<Vec<StorageEntry>, String> {
    let entries = fs::read_dir(path).map_err(|error| error.to_string())?;
    let children = entries
        // One unreadable child must not fail the whole listing.
        .flatten()
        .filter_map(|entry| {
            // `DirEntry::metadata` does not traverse symlinks, so a symlinked
            // folder came out as a ~10-byte "file" that could not be opened or
            // drilled into. `path().metadata()` resolves the link.
            let metadata = entry.path().metadata().ok()?;
            Some(StorageEntry {
                id: u32::MAX,
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                // A folder's own size is unknown until something measures it.
                // `0` is the honest answer, not a placeholder for a `du` this
                // listing is not going to run.
                bytes: if metadata.is_dir() { 0 } else { metadata.len() },
                entries: 1,
            })
        })
        .collect();
    Ok(children)
}
