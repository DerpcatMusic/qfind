use std::cell::{Cell, RefCell};
use std::f64::consts::{PI, TAU};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use qfind_core::{Catalog, ChartScope, ManagerSession, StorageEntry, StorageMap};

use crate::surface::tile_color;

#[derive(Clone)]
struct Arc {
    entry: StorageEntry,
    start: f64,
    end: f64,
    inner: f64,
    outer: f64,
    color: (f64, f64, f64),
}

#[derive(Clone)]
pub struct Pane {
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    title: gtk::Label,
    detail: gtk::Label,
    map: Rc<RefCell<Option<Rc<StorageMap>>>>,
    catalog_map: Rc<RefCell<Option<Rc<StorageMap>>>>,
    current: Rc<Cell<Option<u32>>>,
    hovered: Rc<Cell<Option<u32>>>,
    arcs: Rc<RefCell<Vec<Arc>>>,
    catalog_generation: Rc<Cell<u64>>,
    manager: Rc<RefCell<ManagerSession>>,
    navigate: Rc<RefCell<Box<dyn Fn(PathBuf)>>>,
    select: Rc<RefCell<Box<dyn Fn(&StorageEntry) -> bool>>>,
    context: Rc<RefCell<Box<dyn Fn(&StorageEntry)>>>,
    context_menu: Rc<RefCell<Option<gtk::PopoverMenu>>>,
}

impl Pane {
    pub fn new(manager: Rc<RefCell<ManagerSession>>) -> Self {
        let title = gtk::Label::new(Some("Current directory"));
        title.add_css_class("title-3");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let detail = gtk::Label::new(Some("Opening Catalog…"));
        detail.add_css_class("dim-label");
        detail.set_xalign(0.0);

        let area = gtk::DrawingArea::new();
        area.set_widget_name("qfind-storage-map");
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_focusable(true);

        let map = Rc::new(RefCell::new(None::<Rc<StorageMap>>));
        let catalog_map = Rc::new(RefCell::new(None::<Rc<StorageMap>>));
        let current = Rc::new(Cell::new(None));
        let hovered = Rc::new(Cell::new(None));
        let arcs = Rc::new(RefCell::new(Vec::<Arc>::new()));
        let catalog_generation = Rc::new(Cell::new(0));
        let navigate: Rc<RefCell<Box<dyn Fn(PathBuf)>>> = Rc::new(RefCell::new(Box::new(|_| {})));
        let select: Rc<RefCell<Box<dyn Fn(&StorageEntry) -> bool>>> =
            Rc::new(RefCell::new(Box::new(|_| false)));
        let context: Rc<RefCell<Box<dyn Fn(&StorageEntry)>>> =
            Rc::new(RefCell::new(Box::new(|_| {})));
        let context_menu = Rc::new(RefCell::new(None::<gtk::PopoverMenu>));

        {
            let map = Rc::clone(&map);
            let current = Rc::clone(&current);
            let manager = Rc::clone(&manager);
            let hovered = Rc::clone(&hovered);
            let arcs = Rc::clone(&arcs);
            area.set_draw_func(move |widget, cr, width, height| {
                let fg = widget.color();
                cr.set_source_rgba(
                    f64::from(fg.red()),
                    f64::from(fg.green()),
                    f64::from(fg.blue()),
                    0.035,
                );
                let _ = cr.paint();

                let map_ref = map.borrow();
                let Some(map) = map_ref.as_ref() else {
                    return;
                };
                let radius = f64::from(width.min(height)) * 0.47;
                let cx = f64::from(width) / 2.0;
                let cy = f64::from(height) / 2.0;
                let inner = radius * 0.23;
                let ring = (radius - inner) / 4.0;
                let by_bytes = map.total_bytes() > 0;
                let mut laid_out = Vec::new();
                let manager = manager.borrow();
                let global = manager.chart_scope() == ChartScope::Global;
                let nodes = if global || current.get().is_some() {
                    visible_children(map, current.get(), by_bytes)
                } else {
                    Vec::new()
                };
                layout(
                    map,
                    nodes,
                    -PI / 2.0,
                    -PI / 2.0 + TAU,
                    0,
                    inner,
                    ring,
                    by_bytes,
                    &mut laid_out,
                );
                for arc in &laid_out {
                    let hot = hovered.get() == Some(arc.entry.id);
                    let (r, g, b) = arc.color;
                    cr.set_source_rgb(
                        (r + if hot { 0.16 } else { 0.0 }).min(1.0),
                        (g + if hot { 0.16 } else { 0.0 }).min(1.0),
                        (b + if hot { 0.16 } else { 0.0 }).min(1.0),
                    );
                    cr.move_to(
                        cx + arc.inner * arc.start.cos(),
                        cy + arc.inner * arc.start.sin(),
                    );
                    cr.arc(cx, cy, arc.outer, arc.start, arc.end);
                    cr.line_to(
                        cx + arc.inner * arc.end.cos(),
                        cy + arc.inner * arc.end.sin(),
                    );
                    cr.arc_negative(cx, cy, arc.inner, arc.end, arc.start);
                    cr.close_path();
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(0.02, 0.02, 0.03, 0.72);
                    cr.set_line_width(1.4);
                    let _ = cr.stroke();
                }
                cr.set_font_size(11.0);
                cr.set_source_rgb(0.98, 0.98, 1.0);
                for arc in &laid_out {
                    let mid_radius = (arc.inner + arc.outer) / 2.0;
                    let measure = if arc.entry.bytes > 0 {
                        human_bytes(arc.entry.bytes)
                    } else {
                        format!("{} items", arc.entry.entries)
                    };
                    let Some(label) = fitted_label(
                        cr,
                        &arc.entry.name,
                        &measure,
                        (arc.end - arc.start) * mid_radius,
                    ) else {
                        continue;
                    };
                    let extents = cr.text_extents(&label).ok();
                    let angle = (arc.start + arc.end) / 2.0;
                    let x = cx + mid_radius * angle.cos();
                    let y = cy + mid_radius * angle.sin();
                    cr.move_to(extents.as_ref().map_or(x, |e| x - e.width() / 2.0), y + 4.0);
                    let _ = cr.show_text(&label);
                }
                *arcs.borrow_mut() = laid_out;

                cr.set_source_rgba(
                    f64::from(fg.red()),
                    f64::from(fg.green()),
                    f64::from(fg.blue()),
                    0.9,
                );
                cr.set_font_size((radius * 0.09).clamp(12.0, 18.0));
                let center = if global {
                    "ALL".to_owned()
                } else {
                    current
                        .get()
                        .and_then(|id| map.node(id))
                        .map(|node| node.name)
                        .or_else(|| {
                            manager
                                .directory()
                                .and_then(Path::file_name)
                                .map(|name| name.to_string_lossy().into_owned())
                        })
                        .unwrap_or_else(|| "DIRECTORY".to_owned())
                };
                let extents = cr.text_extents(&center).ok();
                let x = extents.as_ref().map_or(cx, |e| cx - e.width() / 2.0);
                cr.move_to(x, cy + 5.0);
                let _ = cr.show_text(&center);
            });
        }

        let motion = gtk::EventControllerMotion::new();
        {
            let area = area.clone();
            let hovered = Rc::clone(&hovered);
            let arcs = Rc::clone(&arcs);
            let select = Rc::clone(&select);
            motion.connect_motion(move |_, x, y| {
                let entry = hit(&arcs.borrow(), area.width(), area.height(), x, y)
                    .map(|arc| arc.entry.clone());
                let next = entry.as_ref().map(|entry| entry.id);
                if next != hovered.get() {
                    hovered.set(next);
                    if let Some(entry) = entry.as_ref().filter(|entry| entry.id != u32::MAX) {
                        select.borrow()(entry);
                    }
                    area.set_cursor_from_name(next.map(|_| "pointer"));
                    area.set_tooltip_text(
                        next.and_then(|id| {
                            arcs.borrow()
                                .iter()
                                .find(|arc| arc.entry.id == id)
                                .map(|arc| {
                                    format!(
                                        "{}\n{} · {} items",
                                        arc.entry.path.display(),
                                        human_bytes(arc.entry.bytes),
                                        arc.entry.entries
                                    )
                                })
                        })
                        .as_deref(),
                    );
                    area.queue_draw();
                }
            });
        }
        {
            let area = area.clone();
            let hovered = Rc::clone(&hovered);
            motion.connect_leave(move |_| {
                hovered.set(None);
                area.set_cursor_from_name(None);
                area.queue_draw();
            });
        }
        area.add_controller(motion);

        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        {
            let area = area.clone();
            let map = Rc::clone(&map);
            let current = Rc::clone(&current);
            let arcs = Rc::clone(&arcs);
            let title = title.clone();
            let detail = detail.clone();
            let navigate = Rc::clone(&navigate);
            click.connect_released(move |_, _, x, y| {
                let cx = f64::from(area.width()) / 2.0;
                let cy = f64::from(area.height()) / 2.0;
                let inner = f64::from(area.width().min(area.height())) * 0.47 * 0.23;
                if (x - cx).hypot(y - cy) < inner {
                    let parent = map
                        .borrow()
                        .as_ref()
                        .and_then(|map| current.get().and_then(|id| map.node(id)))
                        .and_then(|node| node.path.parent().map(Path::to_path_buf));
                    if let Some(parent) = parent {
                        navigate.borrow()(parent);
                    }
                    return;
                }
                let action = hit(&arcs.borrow(), area.width(), area.height(), x, y)
                    .filter(|arc| arc.entry.is_dir)
                    .and_then(|arc| {
                        map.borrow()
                            .as_ref()
                            .is_some_and(|map| map.has_children(arc.entry.id))
                            .then_some(Some(arc.entry.id))
                    });
                let Some(next) = action else { return };
                if next != current.get() {
                    current.set(next);
                    refresh_labels(&map.borrow(), next, false, None, &title, &detail);
                    area.queue_draw();
                    if let Some(path) = next
                        .and_then(|id| map.borrow().as_ref().and_then(|map| map.node(id)))
                        .map(|node| node.path)
                    {
                        navigate.borrow()(path);
                    }
                }
            });
        }
        area.add_controller(click);

        let right = gtk::GestureClick::new();
        right.set_button(gtk::gdk::BUTTON_SECONDARY);
        {
            let area = area.clone();
            let arcs = Rc::clone(&arcs);
            let select = Rc::clone(&select);
            let context = Rc::clone(&context);
            let context_menu = Rc::clone(&context_menu);
            right.connect_pressed(move |_, _, x, y| {
                let Some(entry) = hit(&arcs.borrow(), area.width(), area.height(), x, y)
                    .map(|arc| arc.entry.clone())
                    .filter(|entry| entry.id != u32::MAX)
                else {
                    return;
                };
                select.borrow()(&entry);
                context.borrow()(&entry);
                if let Some(menu) = context_menu.borrow().as_ref() {
                    crate::surface::popup_at(menu, &area, x, y);
                }
            });
        }
        area.add_controller(right);

        let directory_btn = gtk::ToggleButton::with_label("Directory");
        directory_btn.set_active(true);
        let global_btn = gtk::ToggleButton::with_label("Global");
        global_btn.set_group(Some(&directory_btn));
        let scope = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        scope.add_css_class("linked");
        scope.append(&directory_btn);
        scope.append(&global_btn);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&scope);
        root.append(&title);
        root.append(&detail);
        root.append(&area);
        let pane = Self {
            root,
            area,
            title,
            detail,
            map,
            catalog_map,
            current,
            hovered,
            arcs,
            catalog_generation,
            manager,
            navigate,
            select,
            context,
            context_menu,
        };
        {
            let pane = pane.clone();
            directory_btn.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                pane.manager
                    .borrow_mut()
                    .set_chart_scope(ChartScope::Directory);
                let directory = pane.manager.borrow().directory().map(Path::to_path_buf);
                if let Some(path) = directory {
                    pane.set_directory(&path);
                }
            });
        }
        {
            let pane = pane.clone();
            global_btn.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                pane.manager
                    .borrow_mut()
                    .set_chart_scope(ChartScope::Global);
                pane.current.set(None);
                pane.hovered.set(None);
                pane.arcs.borrow_mut().clear();
                *pane.map.borrow_mut() = pane.catalog_map.borrow().clone();
                refresh_labels(
                    &pane.map.borrow(),
                    None,
                    true,
                    None,
                    &pane.title,
                    &pane.detail,
                );
                pane.area.queue_draw();
            });
        }
        pane
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

    pub fn set_directory(&self, path: &Path) {
        if self.manager.borrow().chart_scope() == ChartScope::Global {
            return;
        }
        let map = self.catalog_map.borrow().clone();
        if let Some(map) = map {
            let next = map.find(path).map(|node| node.id);
            *self.map.borrow_mut() = Some(map);
            self.current.set(next);
            refresh_labels(
                &self.map.borrow(),
                next,
                false,
                Some(path),
                &self.title,
                &self.detail,
            );
            self.area.queue_draw();
        }
    }

    pub fn set_catalog(&self, catalog: Catalog) {
        self.detail.set_text("Building chart…");
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
            let manager = this.manager.borrow();
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
            this.current.set(next);
            this.hovered.set(None);
            this.arcs.borrow_mut().clear();
            *this.catalog_map.borrow_mut() = Some(Rc::clone(&map));
            *this.map.borrow_mut() = Some(map);
            refresh_labels(
                &this.map.borrow(),
                next,
                global,
                directory.as_deref(),
                &this.title,
                &this.detail,
            );
            this.area.queue_draw();
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn layout(
    map: &StorageMap,
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
        out.push(Arc {
            entry: node.clone(),
            start: angle,
            end: next,
            inner: inner + ring * depth as f64,
            outer: inner + ring * (depth + 1) as f64,
            color: tile_color(index + depth * 67, &path),
        });
        if node.is_dir {
            layout(
                map,
                visible_children(map, Some(node.id), by_bytes),
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
        title.set_text(&node.path.display().to_string());
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
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn fitted_label(
    cr: &gtk::cairo::Context,
    name: &str,
    measure: &str,
    available: f64,
) -> Option<String> {
    let fits = |text: &str| {
        cr.text_extents(text)
            .is_ok_and(|extents| extents.width() + 12.0 <= available)
    };
    let full = format!("{name} · {measure}");
    if fits(&full) {
        return Some(full);
    }
    let mut name: Vec<char> = name.chars().collect();
    while !name.is_empty() {
        name.pop();
        let label = format!("{}… · {measure}", name.iter().collect::<String>());
        if fits(&label) {
            return Some(label);
        }
    }
    fits(measure).then(|| measure.to_owned())
}
