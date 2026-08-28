use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use qfind_core::{
    default_snapshot_path, Catalog, Config, DateAge, FileClass, MatchMode, Scope, SearchOpts, Sort,
    Surface, Zoom,
};

mod actions;
mod model;
mod row;
mod settings;
mod surface;
use actions::{
    content_for_path, copy_name, copy_path, copy_uri, open, open_folder, open_with, preview, reveal,
    selected_row,
};
use model::HitModel;
use row::RowData;
use surface::Host;

const APP_ID: &str = "org.qfind.Qfind";
const MAX_ROWS: usize = 5_000;
static QFIND_ROOT: OnceLock<PathBuf> = OnceLock::new();

fn main() -> glib::ExitCode {
    if let Some(root) = parse_here() {
        let _ = QFIND_ROOT.set(root);
    }
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(build_ui);
    let argv = [std::env::args().next().unwrap_or_else(|| "qfind-gtk".into())];
    app.run_with_args(&argv)
}

fn parse_here() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--here" {
            return args.next().filter(|s| !s.is_empty()).map(PathBuf::from);
        }
        if let Some(p) = a.strip_prefix("--here=") {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    std::env::var("QFIND_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn under_root(path: &Path, root: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

struct State {
    catalog: Option<Catalog>,
    model: HitModel,
    else_model: Option<HitModel>,
    here_label: Option<gtk::Label>,
    else_label: Option<gtk::Label>,
    status: gtk::Label,
    search: gtk::SearchEntry,
    scope: Scope,
    class: FileClass,
    sort: Sort,
    match_mode: MatchMode,
    seq: u64,
    snap_mtime: Option<SystemTime>,
    last_ids: Vec<u32>,
    host: Option<Rc<Host>>,
}

fn build_ui(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Qfind")
        .default_width(1100)
        .default_height(700)
        .build();

    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".qfind-odd { background-color: alpha(@theme_fg_color, 0.06); }
         .qfind-odd:selected { background-color: alpha(@theme_selected_bg_color, 1); }
         .qfind-ext { font-weight: 600; opacity: 0.88; }
         .qfind-tile { margin: 1px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let header = gtk::HeaderBar::new();
    let index_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    index_btn.set_tooltip_text(Some("Rebuild Catalog (F5)"));
    header.pack_start(&index_btn);

    let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings_btn.set_tooltip_text(Some("Settings"));
    header.pack_start(&settings_btn);

    let cfg = Config::load();

    let zebra_btn = gtk::ToggleButton::new();
    zebra_btn.set_icon_name("view-list-symbolic");
    zebra_btn.set_tooltip_text(Some("Alternating rows"));
    zebra_btn.set_active(cfg.zebra);

    let folders_btn = gtk::ToggleButton::new();
    folders_btn.set_icon_name("folder-symbolic");
    folders_btn.set_tooltip_text(Some("Folders only"));

    let class_drop = gtk::DropDown::from_strings(&[
        "All types",
        "Images",
        "Audio",
        "Video",
        "Documents",
        "Archives",
    ]);
    class_drop.set_tooltip_text(Some("Filter by FileClass"));

    let sort_drop = gtk::DropDown::from_strings(&[
        "Score",
        "Name A–Z",
        "Name Z–A",
        "Newest",
        "Oldest",
        "Largest",
        "Smallest",
    ]);
    sort_drop.set_tooltip_text(Some("Sort (Newest/Oldest live-stat Hits, like Files)"));

    let match_drop = gtk::DropDown::from_strings(&["Fuzzy", "Substring", "Exact"]);
    match_drop.set_tooltip_text(Some(
        "How loose Query is: Fuzzy (hlo→hello), Substring (contiguous), Exact filename",
    ));
    match_drop.set_selected(match cfg.match_mode {
        MatchMode::Fuzzy => 0,
        MatchMode::Substring => 1,
        MatchMode::Exact => 2,
    });

    let tree_btn = gtk::ToggleButton::new();
    tree_btn.set_icon_name("view-list-tree-symbolic");
    tree_btn.set_tooltip_text(Some("Experimental tree"));

    let weight_btn = gtk::ToggleButton::new();
    weight_btn.set_icon_name("view-grid-symbolic");
    weight_btn.set_tooltip_text(Some("Folder WeightMap (WizTree-style)"));
    weight_btn.set_active(cfg.weight_map);

    let zoom_label = gtk::Label::new(Some("12%"));
    zoom_label.add_css_class("dim-label");
    zoom_label.set_tooltip_text(Some("Ctrl+scroll zooms list ↔ grid"));

    header.pack_end(&zoom_label);
    header.pack_end(&zebra_btn);
    header.pack_end(&weight_btn);
    header.pack_end(&tree_btn);

    window.set_titlebar(Some(&header));

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search files…")
        .hexpand(true)
        .build();

    let chrome = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chrome.set_margin_start(8);
    chrome.set_margin_end(8);
    chrome.set_margin_top(4);
    chrome.set_margin_bottom(4);
    chrome.append(&search);
    chrome.append(&match_drop);
    chrome.append(&sort_drop);
    chrome.append(&class_drop);
    chrome.append(&folders_btn);

    let model = HitModel::new();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let else_model = QFIND_ROOT.get().map(|_| HitModel::new());
    let zebra = Rc::new(Cell::new(cfg.zebra));
    let zoom = Rc::new(Cell::new(Zoom::new(cfg.zoom)));
    let spacing = Rc::new(Cell::new(cfg.spacing));
    let preview_mode = Rc::new(Cell::new(cfg.preview));
    let hovered: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let icons: Rc<RefCell<HashMap<String, gio::Icon>>> = Rc::new(RefCell::new(HashMap::new()));


    let menu = hit_menu();
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);

    let factory = surface::make_list_factory(
        selection.clone(),
        popover.clone(),
        Rc::clone(&hovered),
        Rc::clone(&zebra),
        Rc::clone(&zoom),
        Rc::clone(&spacing),
        Rc::clone(&icons),
    );

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.set_vexpand(true);
    popover.set_parent(&list);

    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::COPY);
    let sel_for_drag = selection.clone();
    drag.connect_prepare(move |_, _, _| {
        let data = sel_for_drag.selected_item()?.downcast::<RowData>().ok()?;
        content_for_path(&data.path())
    });
    list.add_controller(drag);

    let sel_for_open = selection.clone();
    let win_for_open = window.clone();
    list.connect_activate(move |_, _| {
        let Some(data) = sel_for_open.selected_item().and_downcast::<RowData>() else {
            return;
        };
        open(&win_for_open, &data.path());
    });

    let grid_factory = surface::make_grid_factory(
        selection.clone(),
        popover.clone(),
        Rc::clone(&zoom),
        Rc::clone(&icons),
        Rc::clone(&hovered),
    );
    let grid = gtk::GridView::new(Some(selection.clone()), Some(grid_factory));
    grid.set_single_click_activate(true);
    grid.set_max_columns(64);
    grid.set_min_columns(1);
    let sel_for_grid = selection.clone();
    let win_for_grid = window.clone();
    grid.connect_activate(move |_, _| {
        let Some(data) = sel_for_grid.selected_item().and_downcast::<RowData>() else {
            return;
        };
        open(&win_for_grid, &data.path());
    });

    let tree_store = gio::ListStore::new::<RowData>();
    let tree_sel = gtk::SingleSelection::new(Some(tree_store.clone()));
    let tree_toggle: Rc<RefCell<Box<dyn Fn(&str)>>> = Rc::new(RefCell::new(Box::new(|_| {})));
    let tree_collapsed: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    let tree = gtk::ListView::new(
        Some(tree_sel.clone()),
        Some(surface::make_tree_factory(
            Rc::clone(&tree_toggle),
            Rc::clone(&tree_collapsed),
            Rc::clone(&icons),
            Rc::clone(&hovered),
        )),
    );
    let win_tree = window.clone();
    let tree_sel_open = tree_sel.clone();
    tree.connect_activate(move |_, _| {
        if let Some(data) = tree_sel_open.selected_item().and_downcast::<RowData>() {
            open(&win_tree, &data.path());
        }
    });

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    let list_scroll = flick_scroll(&list);
    let grid_scroll = flick_scroll(&grid);
    let tree_scroll = flick_scroll(&tree);

    let mut else_list = None;
    let mut here_label = None;
    let mut else_label = None;
    let list_page: gtk::Widget = if let (Some(root), Some(em)) = (QFIND_ROOT.get(), else_model.as_ref()) {
        let else_sel = gtk::SingleSelection::new(Some(em.clone()));
        let else_factory = surface::make_list_factory(
            else_sel.clone(),
            popover.clone(),
            Rc::clone(&hovered),
            Rc::clone(&zebra),
            Rc::clone(&zoom),
            Rc::clone(&spacing),
            Rc::clone(&icons),
        );
        let elist = gtk::ListView::new(Some(else_sel.clone()), Some(else_factory));
        elist.set_vexpand(true);
        let win_else = window.clone();
        let else_sel_open = else_sel.clone();
        elist.connect_activate(move |_, _| {
            if let Some(data) = else_sel_open.selected_item().and_downcast::<RowData>() {
                open(&win_else, &data.path());
            }
        });
        let hl = gtk::Label::new(Some(&format!("In {}", root.display())));
        hl.set_xalign(0.0);
        hl.add_css_class("heading");
        hl.set_margin_start(10);
        hl.set_margin_top(6);
        let el = gtk::Label::new(Some("Elsewhere"));
        el.set_xalign(0.0);
        el.add_css_class("heading");
        el.set_margin_start(10);
        el.set_margin_top(6);
        let top = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top.append(&hl);
        top.append(&list_scroll);
        let bot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bot.append(&el);
        bot.append(&flick_scroll(&elist));
        let paned = gtk::Paned::new(gtk::Orientation::Vertical);
        paned.set_start_child(Some(&top));
        paned.set_end_child(Some(&bot));
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(true);
        paned.set_wide_handle(true);
        paned.set_position(280);
        paned.set_vexpand(true);
        here_label = Some(hl);
        else_label = Some(el);
        else_list = Some(elist);
        paned.upcast()
    } else {
        list_scroll.clone().upcast()
    };

    stack.add_named(&list_page, Some("list"));
    stack.add_named(&grid_scroll, Some("grid"));
    stack.add_named(&tree_scroll, Some("tree"));

    let tiles: Rc<RefCell<Vec<qfind_core::Tile>>> = Rc::new(RefCell::new(Vec::new()));
    let weight = surface::make_weight_area(Rc::clone(&tiles));

    let host = Rc::new(Host {
        root: gtk::Box::new(gtk::Orientation::Vertical, 0),
        stack: stack.clone(),
        list: list.clone(),
        else_list: else_list.clone(),
        grid: grid.clone(),
        tree: tree.clone(),
        tree_store,
        weight: weight.clone(),
        zoom_label: zoom_label.clone(),
        zoom: Rc::clone(&zoom),
        spacing: Rc::clone(&spacing),
        surface: Rc::new(Cell::new(Surface::Auto)),
        show_weight: Rc::new(Cell::new(cfg.weight_map)),
        collapsed: Rc::clone(&tree_collapsed),
        tree_src: RefCell::new(None),
        tiles,
    });
    {
        let host = Rc::clone(&host);
        *tree_toggle.borrow_mut() = Box::new(move |path| surface::toggle_fold(&host, path));
    }
    host.apply();

    let status = gtk::Label::new(Some("Opening Catalog…"));
    status.set_xalign(0.0);
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    status.set_margin_start(8);
    status.set_margin_end(8);
    status.set_margin_top(4);
    status.set_margin_bottom(4);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&chrome);
    vbox.append(&stack);
    vbox.append(&weight);
    vbox.append(&status);
    window.set_child(Some(&vbox));

    let preview_slot: Rc<RefCell<Option<gtk::Window>>> = Rc::new(RefCell::new(None));
    surface::attach_preview_on_hits(
        &list,
        selection.clone(),
        window.clone(),
        Rc::clone(&preview_slot),
        Rc::clone(&hovered),
        Rc::clone(&preview_mode),
    );
    surface::attach_preview_on_hits(
        &grid,
        selection.clone(),
        window.clone(),
        Rc::clone(&preview_slot),
        Rc::clone(&hovered),
        Rc::clone(&preview_mode),
    );
    surface::attach_preview_on_hits(
        &tree,
        tree_sel.clone(),
        window.clone(),
        Rc::clone(&preview_slot),
        Rc::clone(&hovered),
        Rc::clone(&preview_mode),
    );
    if let Some(elist) = host.else_list.as_ref() {
        if let Some(m) = elist.model() {
            if let Ok(sel) = m.downcast::<gtk::SingleSelection>() {
                surface::attach_preview_on_hits(
                    elist,
                    sel,
                    window.clone(),
                    Rc::clone(&preview_slot),
                    Rc::clone(&hovered),
                    Rc::clone(&preview_mode),
                );
            }
        }
        surface::attach_zoom_scroll(elist, Rc::clone(&host));
    }
    surface::attach_zoom_scroll(&list_scroll, Rc::clone(&host));
    surface::attach_zoom_scroll(&grid_scroll, Rc::clone(&host));
    surface::attach_zoom_scroll(&tree_scroll, Rc::clone(&host));
    {
        let host = Rc::clone(&host);
        window.connect_realize(move |win| {
            let Some(surface) = win.surface() else {
                return;
            };
            let host = Rc::clone(&host);
            surface.connect_layout(move |_, _, _| {
                host.fit_grid();
                host.fit_names();
            });
        });
    }

    let state = Rc::new(RefCell::new(State {
        catalog: None,
        model: model.clone(),
        else_model,
        here_label,
        else_label,
        status: status.clone(),
        search: search.clone(),
        scope: Scope::All,
        class: FileClass::All,
        sort: Sort::Score,
        match_mode: cfg.match_mode,
        seq: 0,
        snap_mtime: None,
        last_ids: Vec::new(),
        host: Some(Rc::clone(&host)),
    }));

    install_actions(&window, &selection, Rc::clone(&preview_slot));

    {
        let state = Rc::clone(&state);
        search.connect_search_changed(move |_| kick_search(&state));
    }
    {
        let sel = selection.clone();
        let win = window.clone();
        search.connect_activate(move |_| {
            if let Some(row) = selected_row(&sel) {
                open(&win, &row.path());
            }
        });
    }
    {
        let state = Rc::clone(&state);
        folders_btn.connect_toggled(move |btn| {
            state.borrow_mut().scope = if btn.is_active() {
                Scope::Folders
            } else {
                Scope::All
            };
            kick_search(&state);
        });
    }
    {
        let state = Rc::clone(&state);
        class_drop.connect_selected_notify(move |drop| {
            state.borrow_mut().class = match drop.selected() {
                1 => FileClass::Image,
                2 => FileClass::Audio,
                3 => FileClass::Video,
                4 => FileClass::Document,
                5 => FileClass::Archive,
                _ => FileClass::All,
            };
            kick_search(&state);
        });
    }
    {
        let state = Rc::clone(&state);
        sort_drop.connect_selected_notify(move |drop| {
            state.borrow_mut().sort = match drop.selected() {
                1 => Sort::Name,
                2 => Sort::NameDesc,
                3 => Sort::Newest,
                4 => Sort::Oldest,
                5 => Sort::Largest,
                6 => Sort::Smallest,
                _ => Sort::Score,
            };
            kick_search(&state);
        });
    }
    {
        let state = Rc::clone(&state);
        match_drop.connect_selected_notify(move |drop| {
            state.borrow_mut().match_mode = match drop.selected() {
                1 => MatchMode::Substring,
                2 => MatchMode::Exact,
                _ => MatchMode::Fuzzy,
            };
            kick_search(&state);
        });
    }
    {
        let state = Rc::clone(&state);
        let zebra = Rc::clone(&zebra);
        zebra_btn.connect_toggled(move |btn| {
            zebra.set(btn.is_active());
            let n = state.borrow().model.n_items();
            if n > 0 {
                state.borrow().model.items_changed(0, n, n);
            }
            if let Some(em) = state.borrow().else_model.as_ref() {
                let n = em.n_items();
                if n > 0 {
                    em.items_changed(0, n, n);
                }
            }
        });
    }
    {
        let state = Rc::clone(&state);
        let window = window.clone();
        index_btn.connect_clicked(move |_| start_rebuild(&state, &window, true));
    }
    {
        let state = Rc::clone(&state);
        let window = window.clone();
        let zoom = Rc::clone(&zoom);
        let spacing = Rc::clone(&spacing);
        let preview_mode = Rc::clone(&preview_mode);
        let zebra = Rc::clone(&zebra);
        let match_live = Rc::new(Cell::new(cfg.match_mode));
        settings_btn.connect_clicked(move |_| {
            let host = state.borrow().host.clone();
            let weight = host
                .as_ref()
                .map(|h| Rc::clone(&h.show_weight))
                .unwrap_or_else(|| Rc::new(Cell::new(true)));
            let state_rb = Rc::clone(&state);
            let window_rb = window.clone();
            let match_live = Rc::clone(&match_live);
            let match_drop = match_drop.clone();
            settings::open(
                &window,
                settings::Live {
                    zoom: Rc::clone(&zoom),
                    spacing: Rc::clone(&spacing),
                    preview: Rc::clone(&preview_mode),
                    zebra: Rc::clone(&zebra),
                    weight,
                    match_mode: Rc::clone(&match_live),
                    on_rebuild: Box::new(move || {
                        let mode = match_live.get();
                        state_rb.borrow_mut().match_mode = mode;
                        match_drop.set_selected(match mode {
                            MatchMode::Fuzzy => 0,
                            MatchMode::Substring => 1,
                            MatchMode::Exact => 2,
                        });
                        if let Some(h) = state_rb.borrow().host.as_ref() {
                            h.apply();
                        }
                        start_rebuild(&state_rb, &window_rb, true);
                    }),
                },
            );
        });
    }
    {
        let state = Rc::clone(&state);
        tree_btn.connect_toggled(move |btn| {
            if let Some(host) = state.borrow().host.as_ref() {
                host.surface.set(if btn.is_active() {
                    Surface::Tree
                } else {
                    Surface::Auto
                });
                host.apply();
                if let (Some(c), ids) = (state.borrow().catalog.clone(), state.borrow().last_ids.clone()) {
                    surface::rebuild_tree(host, &c, &ids);
                }
            }
        });
    }
    {
        let state = Rc::clone(&state);
        weight_btn.connect_toggled(move |btn| {
            if let Some(host) = state.borrow().host.as_ref() {
                host.show_weight.set(btn.is_active());
                host.apply();
            }
        });
    }

    {
        let sel = selection.clone();
        let state = Rc::clone(&state);
        selection.connect_selected_notify(move |_| {
            let st = state.borrow();
            let n = st.model.n_items();
            let extra = selected_row(&sel)
                .map(|r| format!("  ·  {}", r.path()))
                .unwrap_or_default();
            if let Some(c) = &st.catalog {
                st.status.set_text(&format!(
                    "{n} Hits  ·  {} folders · {} files{extra}",
                    c.folder_count(),
                    c.file_count()
                ));
            }
        });
    }

    install_keys(
        &window,
        &search,
        &list,
        &selection,
        Rc::clone(&preview_slot),
        Rc::clone(&hovered),
        Rc::clone(&preview_mode),
        Rc::clone(&state),
        popover.clone(),
    );

    window.present();
    start_rebuild(&state, &window, false);
    start_poll(&state);
}

fn hit_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let open = gio::Menu::new();
    open.append(Some("Open"), Some("win.open"));
    open.append(Some("Open With…"), Some("win.open-with"));
    open.append(Some("Preview"), Some("win.preview"));
    menu.append_section(None, &open);
    let place = gio::Menu::new();
    place.append(Some("Show in Files"), Some("win.reveal"));
    place.append(Some("Open Folder"), Some("win.open-folder"));
    menu.append_section(None, &place);
    let clip = gio::Menu::new();
    clip.append(Some("Copy Path"), Some("win.copy-path"));
    clip.append(Some("Copy Name"), Some("win.copy-name"));
    clip.append(Some("Copy URI"), Some("win.copy-uri"));
    menu.append_section(None, &clip);
    menu
}

fn install_actions(
    window: &gtk::ApplicationWindow,
    selection: &gtk::SingleSelection,
    preview_slot: Rc<RefCell<Option<gtk::Window>>>,
) {
    let add = |name: &str, f: Box<dyn Fn(RowData) + 'static>| {
        let act = gio::SimpleAction::new(name, None);
        let sel = selection.clone();
        act.connect_activate(move |_, _| {
            if let Some(row) = selected_row(&sel) {
                f(row);
            }
        });
        window.add_action(&act);
    };

    let win = window.clone();
    add(
        "open",
        Box::new(move |row| open(&win, &row.path())),
    );
    let win = window.clone();
    add(
        "open-with",
        Box::new(move |row| open_with(&win, &row.path())),
    );
    let win = window.clone();
    add(
        "reveal",
        Box::new(move |row| reveal(&win, &row.path())),
    );
    let win = window.clone();
    add(
        "open-folder",
        Box::new(move |row| open_folder(&win, &row.path(), row.is_dir())),
    );
    add("copy-path", Box::new(|row| copy_path(&row.path())));
    add("copy-name", Box::new(|row| copy_name(&row.name())));
    add("copy-uri", Box::new(|row| copy_uri(&row.path())));

    let act = gio::SimpleAction::new("preview", None);
    let sel = selection.clone();
    let win = window.clone();
    act.connect_activate(move |_, _| {
        if let Some(row) = selected_row(&sel) {
            preview(win.upcast_ref(), &row.path(), &preview_slot);
        }
    });
    window.add_action(&act);
}

fn focus_in(window: &gtk::ApplicationWindow, ancestor: &impl IsA<gtk::Widget>) -> bool {
    let Some(focus) = gtk::prelude::RootExt::focus(window) else {
        return false;
    };
    let target = ancestor.upcast_ref::<gtk::Widget>();
    let mut w = Some(focus);
    while let Some(n) = w {
        if n == *target {
            return true;
        }
        w = n.parent();
    }
    false
}

fn install_keys(
    window: &gtk::ApplicationWindow,
    search: &gtk::SearchEntry,
    list: &gtk::ListView,
    selection: &gtk::SingleSelection,
    preview_slot: Rc<RefCell<Option<gtk::Window>>>,
    hovered: Rc<RefCell<Option<String>>>,
    preview_mode: Rc<Cell<qfind_core::PreviewMode>>,
    state: Rc<RefCell<State>>,
    popover: gtk::PopoverMenu,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let search = search.clone();
    let list = list.clone();
    let selection = selection.clone();
    let host = window.clone();
    let window = window.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let search_focus = focus_in(&window, &search);
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);

        if key == gdk::Key::space || key == gdk::Key::KP_Space {
            if preview_slot.borrow().is_some() {
                if let Some(w) = preview_slot.borrow_mut().take() {
                    w.close();
                }
                return glib::Propagation::Stop;
            }
            let hovering = hovered.borrow().is_some();
            if search_focus && !hovering {
                return glib::Propagation::Proceed;
            }
            if let Some(path) = surface::preview_path(preview_mode.get(), &hovered, &selection) {
                preview(window.upcast_ref(), &path, &preview_slot);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }

        if key == gdk::Key::Escape {
            if preview_slot.borrow().is_some() {
                if let Some(w) = preview_slot.borrow_mut().take() {
                    w.close();
                }
                return glib::Propagation::Stop;
            }
            if !search.text().is_empty() {
                search.set_text("");
                search.grab_focus();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }

        if key == gdk::Key::F5 {
            start_rebuild(&state, &window, true);
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::Down && search_focus {
            list.grab_focus();
            return glib::Propagation::Stop;
        }

        if (key == gdk::Key::f || key == gdk::Key::l || key == gdk::Key::k) && ctrl {
            search.grab_focus();
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::plus || key == gdk::Key::equal {
            if let Some(host) = state.borrow().host.as_ref() {
                host.zoom.set(host.zoom.get().bump(1));
                host.apply();
            }
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::minus {
            if let Some(host) = state.borrow().host.as_ref() {
                host.zoom.set(host.zoom.get().bump(-1));
                host.apply();
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::Return && ctrl {
            if let Some(row) = selected_row(&selection) {
                reveal(&window, &row.path());
            }
            return glib::Propagation::Stop;
        }

        if (key == gdk::Key::c || key == gdk::Key::C) && ctrl && !search_focus {
            if let Some(row) = selected_row(&selection) {
                if shift {
                    copy_name(&row.name());
                } else {
                    copy_path(&row.path());
                }
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::Menu || (key == gdk::Key::F10 && shift) {
            popover.popup();
            return glib::Propagation::Stop;
        }

        glib::Propagation::Proceed
    });
    host.add_controller(keys);
}

fn flick_scroll(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(child)
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Always)
        .overlay_scrolling(false)
        .kinetic_scrolling(true)
        .build()
}

fn opts_from(state: &State) -> SearchOpts {
    SearchOpts {
        scope: state.scope,
        class: state.class,
        sort: state.sort,
        date: DateAge::Any,
        limit: MAX_ROWS,
        highlight: false,
        match_mode: state.match_mode,
    }
}

fn kick_search(state: &Rc<RefCell<State>>) {
    let seq = {
        let mut st = state.borrow_mut();
        st.seq += 1;
        st.seq
    };
    let state = Rc::clone(state);
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if state.borrow().seq != seq {
            return glib::ControlFlow::Break;
        }
        spawn_search(&state, seq);
        glib::ControlFlow::Break
    });
}

fn spawn_search(state: &Rc<RefCell<State>>, seq: u64) {
    let st = state.borrow();
    let Some(catalog) = st.catalog.clone() else {
        return;
    };
    let q = st.search.text().to_string();
    let opts = opts_from(&st);
    drop(st);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(catalog.search_with(&q, opts).map(|h| {
            let ids = h.ids().to_vec();
            let n = ids.len();
            (ids, n)
        }));
    });
    let state = Rc::clone(state);
    glib::timeout_add_local(Duration::from_millis(8), move || match rx.try_recv() {
        Ok(result) => {
            if state.borrow().seq != seq {
                return glib::ControlFlow::Break;
            }
            match result {
                Ok((ids, n)) => {
                    let (host, catalog) = {
                        let st = state.borrow();
                        (st.host.clone(), st.catalog.clone())
                    };
                    state.borrow_mut().last_ids = ids.clone();
                    if let Some(root) = QFIND_ROOT.get() {
                        let (here_ids, else_ids) = partition_here(&catalog, &ids, root);
                        let here_n = here_ids.len();
                        let else_n = else_ids.len();
                        state.borrow().model.set_ids(here_ids);
                        if let Some(em) = state.borrow().else_model.as_ref() {
                            em.set_ids(else_ids);
                        }
                        if let Some(l) = state.borrow().here_label.as_ref() {
                            l.set_text(&format!("In {}  ·  {here_n}", root.display()));
                        }
                        if let Some(l) = state.borrow().else_label.as_ref() {
                            l.set_text(&format!("Elsewhere  ·  {else_n}"));
                        }
                    } else {
                        state.borrow().model.set_ids(ids.clone());
                    }
                    if let (Some(host), Some(c)) = (host, catalog) {
                        surface::rebuild_tree(&host, &c, &ids);
                        surface::rebuild_weight(&host, &c, &ids);
                        host.apply();
                    }
                    let st = state.borrow();
                    if let Some(c) = &st.catalog {
                        st.status.set_text(&format!(
                            "{n} Hits  ·  {} folders · {} files",
                            c.folder_count(),
                            c.file_count()
                        ));
                    }
                }
                Err(err) => state.borrow().status.set_text(&format!("{err}")),
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

fn partition_here(catalog: &Option<Catalog>, ids: &[u32], root: &Path) -> (Vec<u32>, Vec<u32>) {
    let Some(catalog) = catalog else {
        return (ids.to_vec(), Vec::new());
    };
    let mut here = Vec::new();
    let mut rest = Vec::new();
    for &id in ids {
        match catalog.hit(id) {
            Some(hit) if under_root(hit.path().as_path(), root) => here.push(id),
            Some(_) => rest.push(id),
            None => {}
        }
    }
    (here, rest)
}

fn adopt_catalog(state: &Rc<RefCell<State>>, catalog: Catalog) {
    let mtime = std::fs::metadata(catalog.path())
        .and_then(|m| m.modified())
        .ok();
    state.borrow().model.set_catalog(catalog.clone());
    if let Some(em) = state.borrow().else_model.as_ref() {
        em.set_catalog(catalog.clone());
    }
    {
        let mut st = state.borrow_mut();
        st.snap_mtime = mtime;
        st.status.set_text(&format!(
            "Catalog ready  ·  {} folders · {} files",
            catalog.folder_count(),
            catalog.file_count()
        ));
        st.catalog = Some(catalog.clone());
    }
    let warm = catalog;
    thread::spawn(move || warm.warm());
    kick_search(state);
}

fn start_rebuild(state: &Rc<RefCell<State>>, window: &gtk::ApplicationWindow, force: bool) {
    let snapshot = default_snapshot_path();
    if !force && snapshot.exists() {
        if let Ok(catalog) = Catalog::open(&snapshot) {
            adopt_catalog(state, catalog);
            return;
        }
    }

    state
        .borrow()
        .status
        .set_text("Rebuilding Catalog from local Mounts…");
    window.set_sensitive(false);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(Catalog::rebuild(Config::load().rebuild()));
    });
    let state = Rc::clone(state);
    let window = window.clone();
    glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
        Ok(Ok(catalog)) => {
            window.set_sensitive(true);
            adopt_catalog(&state, catalog);
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            window.set_sensitive(true);
            state
                .borrow()
                .status
                .set_text(&format!("rebuild failed: {err}"));
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            window.set_sensitive(true);
            glib::ControlFlow::Break
        }
    });
}

fn start_poll(state: &Rc<RefCell<State>>) {
    let state = Rc::clone(state);
    glib::timeout_add_local(Duration::from_secs(2), move || {
        let path = default_snapshot_path();
        let Ok(meta) = std::fs::metadata(&path) else {
            return glib::ControlFlow::Continue;
        };
        let Ok(mtime) = meta.modified() else {
            return glib::ControlFlow::Continue;
        };
        let prev = state.borrow().snap_mtime;
        if prev == Some(mtime) || prev.is_none() {
            if prev.is_none() {
                state.borrow_mut().snap_mtime = Some(mtime);
            }
            return glib::ControlFlow::Continue;
        }
        if let Ok(catalog) = Catalog::open(&path) {
            adopt_catalog(&state, catalog);
            state
                .borrow()
                .status
                .set_text("Catalog reloaded (snapshot changed)");
        }
        glib::ControlFlow::Continue
    });
}
