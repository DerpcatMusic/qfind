use qfind_core::{LiveEntry, live_children};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::thread;
use std::time::{Duration, SystemTime};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use qfind_core::{
    BrowseMode, Catalog, CatalogFolder, ChartScope, Config, DateAge, FileClass,
    LocationScope, ManagerSession, MatchMode, Scope, SearchOpts, Sort, Surface, Zoom,
    default_snapshot_path,
};

mod actions;
mod archive;
mod glpie;
mod git_panel;
mod folder_sizes;
mod project_workspace;
mod columns;
mod icons;
mod model;
mod manager_tools;
mod row;
mod settings;
mod storage;
mod surface;
use actions::{
    copy_name, copy_paths, copy_text, open, open_folder,
    open_with, preview, preview_widget, reveal, selected_row, selected_rows,
};
use model::HitModel;
use row::RowData;
use surface::Host;

const APP_ID: &str = "org.qfind.Qfind";
const MAX_ROWS: usize = 5_000;
static QFIND_ROOT: OnceLock<PathBuf> = OnceLock::new();
static FOLDERS_FIRST: AtomicBool = AtomicBool::new(true);
type Navigator = Rc<RefCell<Box<dyn Fn(PathBuf)>>>;


enum SearchResult {
    Indexed(Vec<u32>),
    Live(Vec<LiveEntry>),
}

/// One reversible file operation for Ctrl+Z (cap: [`MAX_UNDO`]).
#[derive(Clone, Debug)]
enum UndoEntry {
    Trash { staged: PathBuf, orig: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
}

impl UndoEntry {
    fn undo(self) -> Result<String, String> {
        match self {
            Self::Trash { staged, orig } => qfind_core::restore(&staged, &orig)
                .map(|_| format!("Restored {}", orig.display()))
                .map_err(|error| error.to_string()),
            Self::Rename { from, to } => qfind_core::rename(&to, &from)
                .map(|_| format!("Restored {}", from.display()))
                .map_err(|error| error.to_string()),
            Self::Created { path } => qfind_core::delete(&path)
                .map(|_| format!("Removed {}", path.display()))
                .map_err(|error| error.to_string()),
        }
    }
}

/// Depth of the Ctrl+Z stack; older entries drop off the front.
const MAX_UNDO: usize = 32;

fn main() -> glib::ExitCode {
    glib::set_application_name("Megaman");
    if let Some(root) = parse_here() {
        let _ = QFIND_ROOT.set(root.canonicalize().unwrap_or(root));
    }
    let protected = QFIND_ROOT.get().cloned();
    thread::spawn(move || archive::prune_cache(protected.as_deref()));
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(build_ui);
    let argv = [std::env::args()
        .next()
        .unwrap_or_else(|| "qfind-gtk".into())];
    app.run_with_args(&argv)
}

fn activate_path(window: &gtk::ApplicationWindow, navigate: &Navigator, path: String) {
    if !archive::is_archive(Path::new(&path)) {
        open(window, &path);
        return;
    }
    let window = window.clone();
    let navigate = Rc::clone(navigate);
    glib::MainContext::default().spawn_local(async move {
        let result = gio::spawn_blocking(move || archive::unpack(Path::new(&path))).await;
        match result {
            Ok(Ok(directory)) => navigate.borrow()(directory),
            Ok(Err(error)) => gtk::AlertDialog::builder()
                .message("Could not open archive")
                .detail(error.to_string())
                .build()
                .show(Some(&window)),
            Err(_) => gtk::AlertDialog::builder()
                .message("Archive worker failed")
                .build()
                .show(Some(&window)),
        }
    });
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn bookmark_file() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".config/qfind/bookmarks"))
}

fn qfind_bookmarks() -> Vec<PathBuf> {
    bookmark_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| {
            text.lines()
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn bookmarks() -> Vec<PathBuf> {
    let mut paths = qfind_bookmarks();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")));
    if let Some(config_home) = config_home {
        for file in [
            config_home.join("gtk-3.0/bookmarks"),
            config_home.join("gtk-4.0/bookmarks"),
        ] {
            if let Ok(text) = std::fs::read_to_string(file) {
                paths.extend(text.lines().filter_map(|line| {
                    let uri = line.split_whitespace().next()?;
                    gio::File::for_uri(uri).path().filter(|path| path.is_dir())
                }));
            }
        }
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")));
    if let Some(data_home) = data_home
        && let Ok(text) = std::fs::read_to_string(data_home.join("user-places.xbel"))
    {
        paths.extend(text.split("href=\"").skip(1).filter_map(|tail| {
            let uri = tail.split('"').next()?;
            gio::File::for_uri(uri).path().filter(|path| path.is_dir())
        }));
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

fn set_bookmark(path: &PathBuf, active: bool) -> std::io::Result<()> {
    let Some(file) = bookmark_file() else {
        return Ok(());
    };
    let mut saved = qfind_bookmarks();
    saved.retain(|candidate| candidate != path);
    if active {
        saved.push(path.clone());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        file,
        saved
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn place_button(name: &str, path: PathBuf, icon: &str, navigate: &Navigator) -> gtk::Button {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class(icon);
    row.append(&image);
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_tooltip_text(Some(&path.display().to_string()));
    button.set_child(Some(&row));
    let navigate = Rc::clone(navigate);
    button.connect_clicked(move |_| navigate.borrow()(path.clone()));
    button
}

fn highlight_place(container: &gtk::Box, current: &Path) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.tooltip_text().as_deref() == Some(current.to_string_lossy().as_ref()) {
                button.add_css_class("qfind-place-active");
            } else {
                button.remove_css_class("qfind-place-active");
            }
        }
        child = widget.next_sibling();
    }
}

fn refresh_places(container: &gtk::Box, navigate: &Navigator) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let heading = gtk::Label::new(Some("Places"));
    heading.add_css_class("heading");
    heading.set_xalign(0.0);
    heading.set_margin_start(8);
    heading.set_margin_bottom(4);
    container.append(&heading);

    // Paths already shown above are filtered out of Pinned below, so a
    // folder Nautilus also bookmarks (e.g. Documents) never appears twice.
    let mut shown = HashSet::new();
    if let Some(home) = home_dir() {
        let standard = [
            ("Home", home.clone(), "user-home-symbolic"),
            ("Desktop", home.join("Desktop"), "user-desktop-symbolic"),
            (
                "Documents",
                home.join("Documents"),
                "folder-documents-symbolic",
            ),
            (
                "Downloads",
                home.join("Downloads"),
                "folder-download-symbolic",
            ),
            ("Music", home.join("Music"), "folder-music-symbolic"),
            (
                "Pictures",
                home.join("Pictures"),
                "folder-pictures-symbolic",
            ),
            ("Videos", home.join("Videos"), "folder-videos-symbolic"),
        ];
        for (name, path, icon) in standard {
            if path.is_dir() {
                shown.insert(path.clone());
                container.append(&place_button(name, path, icon, navigate));
            }
        }
    }

    let saved: Vec<PathBuf> = bookmarks()
        .into_iter()
        .filter(|path| !shown.contains(path))
        .collect();
    if !saved.is_empty() {
        let heading = gtk::Label::new(Some("Pinned"));
        heading.add_css_class("heading");
        heading.set_xalign(0.0);
        heading.set_margin_start(8);
        heading.set_margin_top(12);
        heading.set_margin_bottom(4);
        container.append(&heading);
        for path in saved {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Folder");
            container.append(&place_button(
                name,
                path.clone(),
                "folder-symbolic",
                navigate,
            ));
        }
    }

    // Removable drives, mounted volumes, and network shares, refreshed live
    // whenever the volume monitor fires (USB stick plugged in, share mounted).
    let mut devices: Vec<(String, PathBuf, &'static str)> = Vec::new();
    let monitor = gtk::gio::VolumeMonitor::get();
    for mount in monitor
        .connected_drives()
        .iter()
        .flat_map(|drive| drive.volumes())
        .filter_map(|volume| volume.get_mount())
    {
        if let Some(root) = mount.root().path() {
            if shown.insert(root.clone()) {
                let icon = if mount.can_unmount() || mount.can_eject() {
                    "drive-removable-media-symbolic"
                } else {
                    "drive-harddisk-symbolic"
                };
                devices.push((mount.name().to_string(), root, icon));
            }
        }
    }
    for mount in monitor.mounts() {
        if let Some(root) = mount.root().path() {
            if shown.insert(root.clone()) {
                devices.push((mount.name().to_string(), root, "drive-harddisk-symbolic"));
            }
        }
    }
    if !devices.is_empty() {
        let heading = gtk::Label::new(Some("Devices"));
        heading.add_css_class("heading");
        heading.set_xalign(0.0);
        heading.set_margin_start(8);
        heading.set_margin_top(12);
        heading.set_margin_bottom(4);
        container.append(&heading);
        devices.sort_by_key(|device| device.0.clone());
        for (name, path, icon) in devices {
            container.append(&place_button(&name, path, icon, navigate));
        }
    }
}

fn bind_mode_buttons(
    classic: &gtk::ToggleButton,
    qfind: &gtk::ToggleButton,
    on_change: impl Fn(bool) + Clone + 'static,
) {
    let change = on_change.clone();
    classic.connect_toggled(move |button| {
        if button.is_active() {
            change(false);
        }
    });
    qfind.connect_toggled(move |button| {
        if button.is_active() {
            on_change(true);
        }
    });
}

fn bind_preview_controls(toggle: &gtk::ToggleButton, pane: &gtk::Box, close: &gtk::Button) {
    let pane = pane.clone().upcast::<gtk::Widget>();
    toggle.connect_toggled(move |button| pane.set_visible(button.is_active()));
    let toggle = toggle.clone();
    close.connect_clicked(move |_| toggle.set_active(false));
}

struct State {
    catalog: Option<Catalog>,
    folder: Option<CatalogFolder>,
    manager: Rc<RefCell<ManagerSession>>,
    crumbs: gtk::Box,
    back_btn: gtk::Button,
    forward_btn: gtk::Button,
    up_btn: gtk::Button,
    bookmark_btn: gtk::Button,
    archive_save_btn: gtk::Button,
    model: HitModel,
    selection: gtk::MultiSelection,
    status: gtk::Label,
    search: gtk::SearchEntry,
    scope: Scope,
    class: FileClass,
    sort: Sort,
    match_mode: MatchMode,
    seq: u64,
    snap_mtime: Option<SystemTime>,
    last_ids: Vec<u32>,
    visible_folders: usize,
    visible_files: usize,
    host: Option<Rc<Host>>,
    storage: storage::Pane,
    undo: Vec<UndoEntry>,
}

fn build_ui(app: &gtk::Application) {
    build_ui_at(app, QFIND_ROOT.get().cloned().or_else(home_dir));
}

fn build_ui_at(app: &gtk::Application, initial_folder: Option<PathBuf>) {
    let manager = Rc::new(RefCell::new(ManagerSession::new(initial_folder.clone())));
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(initial_folder.as_ref().map(|path| format!("Megaman · {}", path.display())).unwrap_or_else(|| "Megaman".into()))
        .default_width(1320)
        .default_height(820)
        .build();

    icons::install();
    gtk::Window::set_default_icon_name("megaman");
    let css = gtk::CssProvider::new();
    css.load_from_string(include_str!("design.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let header = gtk::HeaderBar::new();
    let brand = gtk::Label::new(Some("Megaman"));
    brand.add_css_class("qfind-brand");
    brand.set_width_chars(17);
    brand.set_xalign(0.0);
    header.pack_start(&brand);
    let address_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    address_bar.add_css_class("qfind-address");
    header.add_css_class("qfind-shell");
    header.add_css_class("qfind-chrome");
    let back_btn = gtk::Button::from_icon_name("go-previous-symbolic");
    back_btn.set_tooltip_text(Some("Back (Alt+Left)"));
    back_btn.set_sensitive(false);
    address_bar.append(&back_btn);

    let forward_btn = gtk::Button::from_icon_name("go-next-symbolic");
    forward_btn.set_tooltip_text(Some("Forward (Alt+Right)"));
    forward_btn.set_sensitive(false);
    address_bar.append(&forward_btn);

    let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
    up_btn.set_tooltip_text(Some("Parent folder (Alt+Up)"));
    up_btn.set_sensitive(
        initial_folder
            .as_ref()
            .and_then(|path| path.parent())
            .is_some(),
    );
    address_bar.append(&up_btn);

    let bookmark_btn = gtk::Button::new();
    let bookmarked = initial_folder
        .as_ref()
        .is_some_and(|path| bookmarks().contains(path));
    bookmark_btn.set_icon_name(if bookmarked {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
    bookmark_btn.set_tooltip_text(Some("Pin this folder"));
    address_bar.append(&bookmark_btn);

    let index_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    index_btn.set_tooltip_text(Some("Refresh folder · rebuild the index in Indexed mode (F5)"));
    address_bar.append(&index_btn);

    let archive_save_btn = gtk::Button::from_icon_name("document-save-symbolic");
    archive_save_btn.set_tooltip_text(Some("Save extracted changes back to the archive (Ctrl+S)"));
    archive_save_btn.set_visible(false);
    address_bar.append(&archive_save_btn);

    let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings_btn.set_tooltip_text(Some("Settings"));


    let cfg = Config::load();
    let zebra = Rc::new(Cell::new(cfg.zebra));
    let zoom = Rc::new(Cell::new(Zoom::new(cfg.zoom)));
    let spacing = Rc::new(Cell::new(cfg.spacing));

    let zebra_btn = gtk::CheckButton::with_label("Alternating rows");
    zebra_btn.set_tooltip_text(Some("Alternating rows"));
    zebra_btn.set_active(cfg.zebra);

    let folders_btn = gtk::ToggleButton::new();
    folders_btn.set_child(Some(&gtk::Label::new(Some("Folders only"))));
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

    let tree_btn = gtk::CheckButton::with_label("Tree view");
    tree_btn.set_tooltip_text(Some("Experimental tree"));

    let preview_btn = gtk::ToggleButton::new();
    preview_btn.set_widget_name("qfind-preview-toggle");
    preview_btn.set_icon_name("view-right-pane-symbolic");
    preview_btn.set_tooltip_text(Some("Show Preview (F3)"));
    preview_btn.set_active(true);

    let zoom_label = gtk::Label::new(Some("12%"));
    zoom_label.add_css_class("dim-label");
    zoom_label.set_tooltip_text(Some("Ctrl+scroll zooms list ↔ grid"));



    // Center cluster: breadcrumb route + search. HeaderBar centers the title
    // widget; crumbs are rebuilt by refresh_crumbs on every navigation.
    let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    crumbs.set_hexpand(true);
    crumbs.set_halign(gtk::Align::Start);

    window.set_titlebar(Some(&header));

    let search_hint = initial_folder
        .as_ref()
        .map(|root| format!("Search in {}…", root.display()))
        .unwrap_or_else(|| "Search files…".into());
    let search = gtk::SearchEntry::builder()
        .placeholder_text(&search_hint)
        .build();
    search.set_width_chars(44);
    search.add_css_class("qfind-search");
    let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    title_box.set_hexpand(true);
    title_box.append(&gtk::Image::from_icon_name("folder-symbolic"));
    title_box.append(&crumbs);
    title_box.add_css_class("qfind-location");
    address_bar.insert_child_after(&title_box, Some(&up_btn));
    header.set_title_widget(Some(&search));

    let classic_btn = gtk::ToggleButton::with_label("Browse");
    classic_btn.set_widget_name("qfind-mode-classic");
    classic_btn.set_active(true);
    let qfind_btn = gtk::ToggleButton::with_label("Indexed");
    qfind_btn.set_widget_name("qfind-mode-qfind");
    qfind_btn.set_group(Some(&classic_btn));
    classic_btn.set_tooltip_text(Some("Immediate items in this folder"));
    qfind_btn.set_tooltip_text(Some("Everything below this folder"));
    let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mode_box.add_css_class("linked");
    mode_box.append(&classic_btn);
    mode_box.append(&qfind_btn);


    let filter_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    filter_box.set_margin_top(10);
    filter_box.set_margin_bottom(10);
    filter_box.set_margin_start(10);
    filter_box.set_margin_end(10);
    for (label, control) in [
        ("Match", match_drop.clone().upcast::<gtk::Widget>()),
        ("Sort", sort_drop.clone().upcast()),
        ("Type", class_drop.clone().upcast()),
    ] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = gtk::Label::new(Some(label));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        row.append(&title);
        row.append(&control);
        filter_box.append(&row);
    }
    let group_heading = gtk::Label::new(Some("Group"));
    group_heading.add_css_class("heading");
    group_heading.set_xalign(0.0);
    filter_box.append(&group_heading);
    let folders_first_btn = gtk::CheckButton::with_label("Folders first");
    folders_first_btn.set_active(true);
    filter_box.append(&folders_first_btn);
    let filter_heading = gtk::Label::new(Some("Filter"));
    filter_heading.add_css_class("heading");
    filter_heading.set_xalign(0.0);
    filter_box.append(&filter_heading);
    filter_box.append(&folders_btn);
    let filter_popover = gtk::Popover::new();
    filter_popover.set_child(Some(&filter_box));
    let filter_btn = gtk::MenuButton::new();
    filter_btn.set_icon_name("view-filter-symbolic");
    filter_btn.set_tooltip_text(Some("Sort, group, and filter"));
    filter_btn.set_popover(Some(&filter_popover));

    let zoom_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    zoom_scale.set_value(f64::from(cfg.zoom));
    zoom_scale.set_draw_value(false);
    zoom_scale.set_hexpand(true);
    let spacing_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 24.0, 1.0);
    spacing_scale.set_value(f64::from(cfg.spacing));
    spacing_scale.set_draw_value(true);
    spacing_scale.set_hexpand(true);
    let view_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    view_box.set_margin_top(10);
    view_box.set_margin_bottom(10);
    view_box.set_margin_start(12);
    view_box.set_margin_end(12);
    let view_heading = gtk::Label::new(Some("View"));
    view_heading.add_css_class("heading");
    view_heading.set_xalign(0.0);
    view_box.append(&view_heading);
    let list_mode_btn = gtk::ToggleButton::with_label("List");
    list_mode_btn.set_active(!Zoom::new(cfg.zoom).is_grid());
    let grid_mode_btn = gtk::ToggleButton::with_label("Grid");
    grid_mode_btn.set_group(Some(&list_mode_btn));
    grid_mode_btn.set_active(Zoom::new(cfg.zoom).is_grid());
    let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mode_row.add_css_class("linked");
    mode_row.append(&list_mode_btn);
    mode_row.append(&grid_mode_btn);

    let zoom_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    zoom_row.append(&gtk::Image::from_icon_name("zoom-out-symbolic"));
    zoom_row.append(&zoom_scale);
    zoom_row.append(&gtk::Image::from_icon_name("zoom-in-symbolic"));
    zoom_row.append(&zoom_label);
    zoom_scale.set_width_request(100);
    zoom_scale.set_hexpand(false);
    let spacing_label = gtk::Label::new(Some("Spacing"));
    spacing_label.set_xalign(0.0);
    view_box.append(&spacing_label);
    view_box.append(&spacing_scale);
    view_box.append(&zebra_btn);
    view_box.append(&tree_btn);
    settings_btn.set_label("Preferences…");
    view_box.append(&settings_btn);
    let preview_view_btn = gtk::CheckButton::with_label("Inspector pane");
    preview_view_btn.set_active(true);
    view_box.append(&preview_view_btn);
    let visibility_heading = gtk::Label::new(Some("Visibility"));
    visibility_heading.add_css_class("heading");
    visibility_heading.set_xalign(0.0);
    view_box.append(&visibility_heading);
    let hidden_btn = gtk::CheckButton::with_label("Hidden files");
    hidden_btn.set_active(cfg.show_hidden);
    let gitignore_btn = gtk::CheckButton::with_label("Respect .gitignore");
    gitignore_btn.set_active(cfg.respect_gitignore);
    let ignore_btn = gtk::CheckButton::with_label("Respect .ignore");
    ignore_btn.set_active(cfg.respect_ignore);
    view_box.append(&hidden_btn);
    view_box.append(&gitignore_btn);
    view_box.append(&ignore_btn);
    let view_popover = gtk::Popover::new();
    view_popover.set_child(Some(&view_box));
    let view_btn = gtk::MenuButton::new();
    view_btn.set_icon_name("open-menu-symbolic");
    view_btn.set_tooltip_text(Some("View settings"));
    view_btn.set_popover(Some(&view_popover));

    view_btn.set_label("View");
    filter_btn.set_label("Filter");
    let tools_menu = gio::Menu::new();
    tools_menu.append(Some("Select by name, extension or type…"), Some("win.select-matching"));
    tools_menu.append(Some("Batch rename…"), Some("win.batch-rename"));
    tools_menu.append(Some("Copy selected to…"), Some("win.batch-copy"));
    tools_menu.append(Some("Move selected to…"), Some("win.batch-move"));
    tools_menu.append(Some("Compress selected…"), Some("win.batch-zip"));
    tools_menu.append(Some("Extract selected archives…"), Some("win.batch-extract"));
    let tools_btn = gtk::MenuButton::builder().label("Selection").menu_model(&tools_menu).build();
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.add_css_class("qfind-toolbar");
    toolbar.append(&mode_box);
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    toolbar.append(&tools_btn);
    let projects_btn = gtk::ToggleButton::with_label(qfind_core::components::title("projects"));

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);
    toolbar.append(&mode_row);
    toolbar.append(&filter_btn);
    toolbar.append(&view_btn);
    toolbar.append(&preview_btn);

    let model = HitModel::new();
    let selection = gtk::MultiSelection::new(Some(model.clone()));
    let preview_mode = Rc::new(Cell::new(cfg.preview));
    let hovered: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let icons: Rc<RefCell<HashMap<String, gio::Icon>>> = Rc::new(RefCell::new(HashMap::new()));

    let (menu, external_actions) = hit_menu();
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    let context_target: Rc<RefCell<Option<RowData>>> = Rc::new(RefCell::new(None));
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    let navigate: Navigator = Rc::new(RefCell::new(Box::new(|_| {})));

    let name_factory = surface::make_name_factory(
        selection.clone(),
        popover.clone(),
        Rc::clone(&hovered),
        Rc::clone(&zebra),
        Rc::clone(&zoom),
        Rc::clone(&spacing),
        Rc::clone(&icons),
    );
    let storage = storage::Pane::new(Rc::clone(&manager));
    let size_factory = surface::make_size_factory(Rc::clone(&zebra), storage.clone());
    let modified_factory = surface::make_modified_factory(Rc::clone(&zebra));

    let list = gtk::ColumnView::new(Some(selection.clone()));
    list.set_vexpand(true);
    list.set_single_click_activate(false);
    list.set_enable_rubberband(true);
    list.set_show_row_separators(false);
    let name_column = gtk::ColumnViewColumn::new(Some("Name"), Some(name_factory));
    name_column.set_expand(true);
    name_column.set_resizable(true);
    name_column.connect_fixed_width_notify(|column| column.set_expand(false));
    let size_column = gtk::ColumnViewColumn::new(Some("Size"), Some(size_factory));
    size_column.set_fixed_width(110);
    size_column.set_resizable(true);
    let modified_column = gtk::ColumnViewColumn::new(Some("Modified"), Some(modified_factory));
    modified_column.set_fixed_width(140);
    modified_column.set_resizable(true);
    list.append_column(&name_column);
    list.append_column(&size_column);
    list.append_column(&modified_column);
    for (title, location, width) in [("Type", false, 120), ("Location", true, 260)] {
        let column = gtk::ColumnViewColumn::new(Some(title), Some(columns::text_factory(location, zebra.clone())));
        column.set_resizable(true);
        column.set_fixed_width(width);
        column.set_visible(false);
        column.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
        list.append_column(&column);
    }
    toolbar.append(&columns::configure(&list, "files"));
    for column in [&name_column, &size_column, &modified_column] {
        column.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
    }
    if let Some(sorter) = list.sorter().and_downcast::<gtk::ColumnViewSorter>() {
        let sort_drop = sort_drop.clone();
        let model = model.clone();
        let selection = selection.clone();
        let list_weak = list.downgrade();
        sorter.connect_changed(move |sorter, _| {
            let Some(column) = sorter.primary_sort_column() else { return; };
            let descending = sorter.primary_sort_order() == gtk::SortType::Descending;
            if matches!(column.title().as_deref(), Some("Type" | "Location")) {
                let paths: Vec<_> = selected_rows(&selection).into_iter().map(|row| row.path()).collect();
                sort_drop.set_selected(0);
                if sorter.primary_sort_column().as_ref() != Some(&column) {
                    if let Some(list) = list_weak.upgrade() { list.sort_by_column(Some(&column), if descending { gtk::SortType::Descending } else { gtk::SortType::Ascending }); }
                }
                model.set_text_sort(Some((column.title().as_deref() == Some("Location"), descending)));
                for path in paths { if let Some(position) = model.position_path(&path) { selection.select_item(position, false); } }
                return;
            }
            model.set_text_sort(None);
            let choice = match column.title().as_deref() {
                Some("Name") => if descending { 2 } else { 1 },
                Some("Size") => if descending { 5 } else { 6 },
                Some("Modified") => if descending { 3 } else { 4 },
                _ => return,
            };
            sort_drop.set_selected(choice);
        });
    }
    popover.set_parent(&list);

    let sel_for_open = selection.clone();
    let win_for_open = window.clone();
    let nav_for_open = Rc::clone(&navigate);
    list.connect_activate(move |_, position| {
        let Some(data) = sel_for_open
            .model()
            .and_then(|model| model.item(position))
            .and_downcast::<RowData>()
        else {
            return;
        };
        if data.is_dir() {
            nav_for_open.borrow()(PathBuf::from(data.path()));
        } else {
            activate_path(&win_for_open, &nav_for_open, data.path());
        }
    });

    let grid_factory = surface::make_grid_factory(
        selection.clone(),
        popover.clone(),
        Rc::clone(&zebra),
        Rc::clone(&zoom),
        Rc::clone(&icons),
        Rc::clone(&hovered),
        storage.clone(),
    );
    let grid = gtk::GridView::new(Some(selection.clone()), Some(grid_factory));
    grid.set_single_click_activate(false);
    grid.set_enable_rubberband(true);
    grid.set_max_columns(64);
    grid.set_min_columns(1);
    let sel_for_grid = selection.clone();
    let win_for_grid = window.clone();
    let nav_for_grid = Rc::clone(&navigate);
    grid.connect_activate(move |_, position| {
        let Some(data) = sel_for_grid
            .model()
            .and_then(|model| model.item(position))
            .and_downcast::<RowData>()
        else {
            return;
        };
        if data.is_dir() {
            nav_for_grid.borrow()(PathBuf::from(data.path()));
        } else {
            activate_path(&win_for_grid, &nav_for_grid, data.path());
        }
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
            Rc::clone(&zebra),
            storage.clone(),
        )),
    );
    tree.set_single_click_activate(false);
    tree.set_enable_rubberband(true);
    let win_tree = window.clone();
    let tree_sel_open = tree_sel.clone();
    let nav_for_tree = Rc::clone(&navigate);
    tree.connect_activate(move |_, _| {
        if let Some(data) = tree_sel_open.selected_item().and_downcast::<RowData>() {
            if data.is_dir() {
                nav_for_tree.borrow()(PathBuf::from(data.path()));
            } else {
                activate_path(&win_tree, &nav_for_tree, data.path());
            }
        }
    });

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    let list_scroll = flick_scroll(&list);
    let grid_scroll = flick_scroll(&grid);
    let tree_scroll = flick_scroll(&tree);

    let list_page: gtk::Widget = list_scroll.clone().upcast();

    stack.add_named(&list_page, Some("list"));
    stack.add_named(&grid_scroll, Some("grid"));
    stack.add_named(&tree_scroll, Some("tree"));

    let weights: Rc<RefCell<Vec<qfind_core::Weighted>>> = Rc::new(RefCell::new(Vec::new()));
    let weight_rev: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let weight = surface::make_weight_area(Rc::clone(&weights), Rc::clone(&weight_rev));

    let host = Rc::new(Host {
        root: gtk::Box::new(gtk::Orientation::Vertical, 0),
        stack: stack.clone(),
        list: list.clone(),
        grid: grid.clone(),
        tree: tree.clone(),
        tree_store,
        weight: weight.clone(),
        weight_rev: Rc::clone(&weight_rev),
        zoom_label: zoom_label.clone(),
        zoom_scale: zoom_scale.clone(),
        apply_pending: Cell::new(false),
        zoom: Rc::clone(&zoom),
        spacing: Rc::clone(&spacing),
        surface: Rc::new(Cell::new(Surface::Auto)),
        // Storage owns the catalog-backed treemap; file searches need no extra size walk.
        show_weight: Rc::new(Cell::new(false)),
        collapsed: Rc::clone(&tree_collapsed),
        tree_src: RefCell::new(None),
        weights,
    });
    {
        let host = Rc::clone(&host);
        *tree_toggle.borrow_mut() = Box::new(move |path| surface::toggle_fold(&host, path));
    }
    {
        let host = Rc::downgrade(&host);
        grid.connect_notify_local(Some("width"), move |_, _| {
            if let Some(host) = host.upgrade() {
                host.fit_grid();
            }
        });
    }
    host.apply();
    {
        let zoom_scale = zoom_scale.clone();
        let tree_btn = tree_btn.clone();
        list_mode_btn.connect_toggled(move |button| {
            if button.is_active() {
                tree_btn.set_active(false);
                zoom_scale.set_value(20.0);
            }
        });
    }
    {
        let zoom_scale = zoom_scale.clone();
        let tree_btn = tree_btn.clone();
        grid_mode_btn.connect_toggled(move |button| {
            if button.is_active() {
                tree_btn.set_active(false);
                zoom_scale.set_value(60.0);
            }
        });
    }

    let status = gtk::Label::new(Some("Opening Catalog…"));
    status.set_xalign(0.0);
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    status.set_margin_start(8);
    status.set_margin_end(8);
    status.set_margin_top(4);
    status.set_margin_bottom(4);

    let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
    results.add_css_class("qfind-content");
    results.append(&address_bar);
    results.append(&toolbar);
    results.append(&stack);

    let places = gtk::Box::new(gtk::Orientation::Vertical, 2);
    places.add_css_class("qfind-chrome");
    places.set_margin_top(8);
    places.set_margin_bottom(8);
    places.set_margin_start(6);
    places.set_margin_end(6);
    refresh_places(&places, &navigate);
    {
        let places = places.clone();
        let navigate = Rc::clone(&navigate);
        let monitor = gtk::gio::VolumeMonitor::get();
        let on_volume_event = {
            let places = places.clone();
            let navigate = Rc::clone(&navigate);
            move |_: &gtk::gio::VolumeMonitor, _: &gtk::glib::Object| {
                refresh_places(&places, &navigate);
            }
        };
        use gtk::gio::prelude::VolumeMonitorExt as _;
        monitor.connect_mount_added({
            let on_volume_event = on_volume_event.clone();
            move |monitor, mount| on_volume_event(monitor, mount.upcast_ref())
        });
        monitor.connect_mount_removed({
            let on_volume_event = on_volume_event.clone();
            move |monitor, mount| on_volume_event(monitor, mount.upcast_ref())
        });
        monitor.connect_volume_added({
            let on_volume_event = on_volume_event.clone();
            move |monitor, volume| on_volume_event(monitor, volume.upcast_ref())
        });
        monitor.connect_volume_removed(move |monitor, volume| {
            on_volume_event(monitor, volume.upcast_ref())
        });
    }
    if let Some(path) = initial_folder.as_deref() {
        highlight_place(&places, path);
    }
    let places_scroll = gtk::ScrolledWindow::builder()
        .child(&places)
        .width_request(190)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    places_scroll.add_css_class("qfind-shell");
    places_scroll.add_css_class("qfind-chrome");
    places_scroll.add_css_class("sidebar");

    let preview_title = gtk::ToggleButton::with_label("Preview");
    preview_title.set_active(true);
    preview_title.add_css_class("flat");
    let chart_title = gtk::ToggleButton::with_label(qfind_core::components::title("storage"));
    chart_title.set_group(Some(&preview_title));
    chart_title.add_css_class("flat");
    let preview_name = gtk::Label::new(Some("Select a file"));
    preview_name.add_css_class("title-3");
    preview_name.set_xalign(0.5);
    preview_name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let preview_path = gtk::Label::new(Some("Choose an item to inspect its contents and details."));
    preview_path.add_css_class("dim-label");
    preview_path.set_xalign(0.5);
    preview_path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let preview_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_content.set_vexpand(true);
    let preview_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    preview_page.set_margin_top(24);
    preview_page.set_margin_bottom(8);
    preview_page.set_margin_start(18);
    preview_page.set_margin_end(18);
    preview_page.append(&preview_name);
    preview_page.append(&preview_path);
    preview_page.append(&preview_content);
    {
        let stack = stack.clone();
        storage.set_hover(move |entry| surface::highlight_path(stack.upcast_ref(), entry.map(|entry| entry.path.as_path())));
    }
    {
        let model = model.clone();
        let selection = selection.clone();
        let list = list.clone();
        let grid = grid.clone();
        let stack = stack.clone();
        let context_target = Rc::clone(&context_target);
        storage.bind_results(
            popover.clone(),
            move |entry| {
                let path = entry.path.to_string_lossy();
                let Some(position) = model
                    .position(entry.id)
                    .or_else(|| model.position_path(&path))
                else {
                    return false;
                };
                selection.select_item(position, true);
                match stack.visible_child_name().as_deref() {
                    Some("grid") => grid.scroll_to(position, gtk::ListScrollFlags::NONE, None),
                    Some("list") => {
                        list.scroll_to(position, None, gtk::ListScrollFlags::NONE, None);
                    }
                    _ => {}
                }
                true
            },
            move |entry| {
                context_target.replace(Some(RowData::new(
                    &entry.name,
                    entry.path.to_string_lossy(),
                    entry.is_dir,
                    entry.bytes,
                    0,
                )));
            },
        );
    }
    if let Some(path) = initial_folder.as_deref() {
        storage.set_directory(path);
    }
    let pane_stack = gtk::Stack::new();
    pane_stack.set_vexpand(true);
    pane_stack.set_vhomogeneous(false);
    pane_stack.set_hhomogeneous(false);
    pane_stack.add_named(&preview_page, Some("preview"));
    pane_stack.add_named(&storage.root, Some("chart"));
    let projects_page = gtk::Box::new(gtk::Orientation::Vertical, 0);

    {
        let pane_stack = pane_stack.clone();
        preview_title.connect_toggled(move |button| {
            if button.is_active() {
                pane_stack.set_visible_child_name("preview");
            }
        });
    }
    {
        let pane_stack = pane_stack.clone();
        chart_title.connect_toggled(move |button| {
            if button.is_active() {
                pane_stack.set_visible_child_name("chart");
            }
        });
    }
    let preview_close = gtk::Button::from_icon_name("window-close-symbolic");
    preview_close.set_widget_name("qfind-preview-close");
    preview_close.set_tooltip_text(Some("Close Preview (F3)"));
    preview_close.add_css_class("flat");
    let preview_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    preview_header.add_css_class("qfind-inspector-tabs");
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tabs.add_css_class("linked");
    tabs.set_halign(gtk::Align::Center);
    tabs.set_hexpand(true);
    tabs.append(&preview_title);
    tabs.append(&chart_title);

    let git_title = gtk::ToggleButton::with_label("Git");
    git_title.set_group(Some(&preview_title));
    git_title.add_css_class("flat");
    tabs.append(&git_title);
    preview_header.append(&tabs);
    preview_header.append(&preview_close);
    // Flush against the paned separator like the left sidebar: the panel
    // edge carries the chrome background with no outer gap, and the pages
    // inside provide the breathing room.
    let preview_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    preview_panel.add_css_class("qfind-chrome");
    preview_panel.add_css_class("qfind-inspector");
    preview_panel.set_widget_name("qfind-preview-pane");
    let inspector_heading = gtk::Label::new(Some("INSPECTOR"));
    inspector_heading.add_css_class("qfind-section-label");
    inspector_heading.set_xalign(0.0);
    preview_panel.append(&inspector_heading);
    preview_panel.append(&preview_header);
    preview_panel.append(&pane_stack);
    bind_preview_controls(&preview_btn, &preview_panel, &preview_close);
    {
        let preview_btn = preview_btn.clone();
        preview_view_btn.connect_toggled(move |button| {
            if preview_btn.is_active() != button.is_active() {
                preview_btn.set_active(button.is_active());
            }
        });
    }
    {
        let preview_view_btn = preview_view_btn.clone();
        preview_btn.connect_toggled(move |button| {
            if preview_view_btn.is_active() != button.is_active() {
                preview_view_btn.set_active(button.is_active());
            }
        });
    }

    let content_preview = gtk::Paned::new(gtk::Orientation::Horizontal);
    content_preview.set_start_child(Some(&results));
    content_preview.set_end_child(Some(&preview_panel));
    content_preview.set_position(740);
    content_preview.set_resize_start_child(true);
    content_preview.set_resize_end_child(true);
    content_preview.set_shrink_end_child(false);

    let browser = gtk::Paned::new(gtk::Orientation::Horizontal);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    sidebar.add_css_class("qfind-sidebar");
    let workspace_heading = gtk::Label::new(Some("WORKSPACE"));
    workspace_heading.add_css_class("qfind-section-label");
    workspace_heading.set_xalign(0.0);
    sidebar.append(&workspace_heading);
    let storage_shortcut = gtk::ToggleButton::with_label("Storage overview");
    storage_shortcut.set_group(Some(&projects_btn));
    storage_shortcut.set_active(true);
    storage_shortcut.add_css_class("flat");
    projects_btn.set_label("Development projects");
    projects_btn.add_css_class("flat");
    for (button, icon, text) in [
        (&storage_shortcut, "drive-harddisk-symbolic", qfind_core::components::title("storage")),
        (&projects_btn, "utilities-terminal-symbolic", qfind_core::components::title("projects")),
    ] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&gtk::Image::from_icon_name(icon));
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        row.append(&label);
        button.set_child(Some(&row));
    }
    sidebar.append(&storage_shortcut);
    sidebar.append(&projects_btn);
    sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    sidebar.append(&places_scroll);
    browser.set_start_child(Some(&sidebar));
    let workspaces = gtk::Stack::new();
    workspaces.set_hhomogeneous(false);
    workspaces.set_vhomogeneous(false);
    workspaces.add_named(&content_preview, Some("storage"));
    workspaces.add_named(&projects_page, Some("projects"));
    browser.set_end_child(Some(&workspaces));
    browser.set_position(204);
    browser.set_resize_start_child(false);
    browser.set_shrink_start_child(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&browser);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("qfind-footer");
    status.set_hexpand(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    footer.append(&status);
    footer.append(&zoom_row);
    vbox.append(&footer);
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
    surface::attach_zoom_scroll(&list_scroll, Rc::clone(&host));
    surface::attach_zoom_scroll(&grid_scroll, Rc::clone(&host));
    surface::attach_zoom_scroll(&tree_scroll, Rc::clone(&host));
    let state = Rc::new(RefCell::new(State {
        catalog: None,
        folder: None,
        manager,
        crumbs: crumbs.clone(),
        back_btn: back_btn.clone(),
        forward_btn: forward_btn.clone(),
        up_btn: up_btn.clone(),
        bookmark_btn: bookmark_btn.clone(),
        archive_save_btn: archive_save_btn.clone(),
        model: model.clone(),
        selection: selection.clone(),
        status: status.clone(),
        search: search.clone(),
        scope: Scope::All,
        class: FileClass::All,
        sort: Sort::Score,
        match_mode: cfg.match_mode,
        seq: 0,
        snap_mtime: None,
        last_ids: Vec::new(),
        visible_folders: 0,
        visible_files: 0,
        host: Some(Rc::clone(&host)),
        storage: storage.clone(),
        undo: Vec::new(),
    }));

    if let Some(root) = initial_folder.as_ref() {
        update_archive_save_button(&archive_save_btn, root);
    }
    let (git_page, git_status) = git_panel::new(state.clone(), None);
    pane_stack.add_named(&git_page, Some("git"));
    footer.append(&git_status);
    {
        let pane_stack = pane_stack.clone();
        git_title.connect_toggled(move |button| {
            if button.is_active() { pane_stack.set_visible_child_name("git"); }
        });
        let preview_btn = preview_btn.clone();
        git_status.connect_clicked(move |_| {
            preview_btn.set_active(true);
            git_title.set_active(true);
        });
    }
    refresh_crumbs(&state);
    {
        let state = Rc::clone(&state);
        let places = places.clone();
        *navigate.borrow_mut() = Box::new(move |path| {
            highlight_place(&places, &path);
            navigate_to(&state, path, true);
        });
    }
    {
        let navigate = Rc::clone(&navigate);
        storage.set_navigate(move |path| navigate.borrow()(path));
    }

    install_actions(
        &window,
        &navigate,
        ActionTarget {
            selection: selection.clone(),
            popover: popover.clone(),
            context_target,
        },
        Rc::clone(&preview_slot),
        external_actions,
        Rc::clone(&state),
    );
    let bind_visibility = |button: &gtk::CheckButton| {
        let state = Rc::clone(&state);
        let window = window.clone();
        let hidden_btn = hidden_btn.clone();
        let gitignore_btn = gitignore_btn.clone();
        let ignore_btn = ignore_btn.clone();
        button.connect_toggled(move |_| {
            let mut cfg = Config::load();
            cfg.show_hidden = hidden_btn.is_active();
            cfg.respect_gitignore = gitignore_btn.is_active();
            cfg.respect_ignore = ignore_btn.is_active();
            let _ = cfg.save();
            start_rebuild(&state, &window, true);
        });
    };
    bind_visibility(&hidden_btn);
    bind_visibility(&gitignore_btn);
    bind_visibility(&ignore_btn);

    {
        let state = Rc::clone(&state);
        search.connect_search_changed(move |_| kick_search(&state));
    }
    {
        let state = Rc::clone(&state);
        back_btn.connect_clicked(move |_| navigate_history(&state, true));
    }
    {
        let state = Rc::clone(&state);
        let window = window.clone();
        archive_save_btn.connect_clicked(move |button| {
            let workspace = state
                .borrow()
                .manager
                .borrow()
                .directory()
                .and_then(archive::workspace);
            let Some(workspace) = workspace else {
                return;
            };
            button.set_sensitive(false);
            state.borrow().status.set_text("Saving archive…");
            let state = Rc::clone(&state);
            let button = button.clone();
            let window = window.clone();
            glib::MainContext::default().spawn_local(async move {
                let name = workspace
                    .source
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "archive".into());
                let result = gio::spawn_blocking(move || archive::repack(&workspace)).await;
                button.set_sensitive(true);
                match result {
                    Ok(Ok(())) => state.borrow().status.set_text(&format!("Saved {name}")),
                    Ok(Err(error)) => {
                        state.borrow().status.set_text("Archive save failed");
                        gtk::AlertDialog::builder()
                            .message("Could not save archive")
                            .detail(error.to_string())
                            .build()
                            .show(Some(&window));
                    }
                    Err(_) => {
                        state.borrow().status.set_text("Archive save worker failed");
                        gtk::AlertDialog::builder()
                            .message("Archive save worker failed")
                            .build()
                            .show(Some(&window));
                    }
                }
            });
        });
    }
    {
        let state = Rc::clone(&state);
        forward_btn.connect_clicked(move |_| navigate_history(&state, false));
    }
    {
        let state = Rc::clone(&state);
        up_btn.connect_clicked(move |_| navigate_parent(&state));
    }
    {
        let state = Rc::clone(&state);
        let places = places.clone();
        let navigate = Rc::clone(&navigate);
        bookmark_btn.connect_clicked(move |button| {
            let Some(path) = state
                .borrow()
                .manager
                .borrow()
                .directory()
                .map(Path::to_path_buf)
            else {
                return;
            };
            let active = !bookmarks().contains(&path);
            if set_bookmark(&path, active).is_ok() {
                button.set_icon_name(if active {
                    "starred-symbolic"
                } else {
                    "non-starred-symbolic"
                });
                refresh_places(&places, &navigate);
                highlight_place(&places, &path);
            }
        });
    }
    {
        let state = Rc::clone(&state);
        bind_mode_buttons(&classic_btn, &qfind_btn, move |recursive| {
            state.borrow().manager.borrow_mut().set_mode(if recursive {
                BrowseMode::Qfind
            } else {
                BrowseMode::Classic
            });
            kick_search(&state);
        });
    }
    {
        let sel = selection.clone();
        let win = window.clone();
        let navigate = Rc::clone(&navigate);
        search.connect_activate(move |_| {
            if let Some(row) = selected_row(&sel) {
                if row.is_dir() {
                    navigate.borrow()(PathBuf::from(row.path()));
                } else {
                    activate_path(&win, &navigate, row.path());
                }
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
        folders_first_btn.connect_toggled(move |btn| {
            FOLDERS_FIRST.store(btn.is_active(), AtomicOrdering::Relaxed);
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
        let list = list.clone();
        sort_drop.connect_selected_notify(move |drop| {
            let sort = match drop.selected() {
                1 => Sort::Name,
                2 => Sort::NameDesc,
                3 => Sort::Newest,
                4 => Sort::Oldest,
                5 => Sort::Largest,
                6 => Sort::Smallest,
                _ => Sort::Score,
            };
            if state.borrow().sort == sort {
                return;
            }
            state.borrow().model.set_text_sort(None);
            state.borrow_mut().sort = sort;
            let (title, order) = match sort {
                Sort::Name => ("Name", gtk::SortType::Ascending),
                Sort::NameDesc => ("Name", gtk::SortType::Descending),
                Sort::Largest => ("Size", gtk::SortType::Descending),
                Sort::Smallest => ("Size", gtk::SortType::Ascending),
                Sort::Newest => ("Modified", gtk::SortType::Descending),
                Sort::Oldest => ("Modified", gtk::SortType::Ascending),
                _ => ("", gtk::SortType::Ascending),
            };
            let column = (0..list.columns().n_items()).filter_map(|i| list.columns().item(i).and_downcast::<gtk::ColumnViewColumn>()).find(|column| column.title().as_deref() == Some(title));
            list.sort_by_column(column.as_ref(), order);
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
        });
    }
    {
        let state = Rc::clone(&state);
        let window = window.clone();
        index_btn.connect_clicked(move |_| refresh_current(&state, &window));
    }
    {
        let state = Rc::clone(&state);
        let window = window.clone();
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
                    preview: Rc::clone(&preview_mode),
                    zebra: Rc::clone(&zebra),
                    weight,
                    match_mode: Rc::clone(&match_live),
                    on_save: Box::new(move |rebuild| {
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
                        if rebuild {
                            start_rebuild(&state_rb, &window_rb, true);
                        } else {
                            kick_search(&state_rb);
                        }
                    }),
                },
            );
        });
    }
    {
        let state = Rc::clone(&state);
        tree_btn.connect_toggled(move |btn| {
            let active = btn.is_active();
            let st = state.borrow();
            if let Some(host) = st.host.as_ref() {
                host.surface
                    .set(if active { Surface::Tree } else { Surface::Auto });
                if active && let Some(c) = st.catalog.as_ref() {
                    let ids = st.last_ids.clone();
                    surface::rebuild_tree(host, c, &ids);
                }
                host.apply();
            }
        });
    }
    {
        let host = Rc::clone(&host);
        zoom_scale.connect_value_changed(move |scale| {
            let next = Zoom::new(scale.value() as u8);
            if next == host.zoom.get() { return; }
            host.zoom.set(next);
            host.schedule_apply();
        });
    }
    {
        let host = Rc::clone(&host);
        spacing_scale.connect_value_changed(move |scale| {
            host.spacing.set(scale.value() as u8);
            host.apply();
        });
    }
    {
        let zoom = Rc::clone(&zoom);
        let spacing = Rc::clone(&spacing);
        view_popover.connect_closed(move |_| {
            let mut cfg = Config::load();
            cfg.zoom = zoom.get().get();
            cfg.spacing = spacing.get();
            let _ = cfg.save();
        });
    }
    {
        let global = gio::SimpleAction::new("global-search", None);
        let storage_button = storage_shortcut.clone();
        let indexed = qfind_btn.clone();
        let sort = sort_drop.clone();
        let search = search.clone();
        let state = state.clone();
        let list = list.clone();
        global.connect_activate(move |_, _| {
            storage_button.emit_clicked();
            state.borrow().model.set_text_sort(None);
            sort.set_selected(0);
            list.sort_by_column(None::<&gtk::ColumnViewColumn>, gtk::SortType::Ascending);
            indexed.set_active(true);
            state.borrow().manager.borrow_mut().set_search_scope(LocationScope::Global);
            search.set_placeholder_text(Some("Search everywhere…"));
            search.grab_focus();
            search.select_region(0, -1);
            kick_search(&state);
        });
        window.add_action(&global);
        app.set_accels_for_action("win.global-search", &["<Control>g"]);
    }
    {
        let workspace = workspaces.clone();
        let places_scroll = places_scroll.clone();
        let search = search.clone();
        let zoom_row = zoom_row.clone();
        let footer = footer.clone();
        storage_shortcut.connect_clicked(move |_| {
            workspace.set_visible_child_name("storage");
            places_scroll.set_visible(true);
            search.set_visible(true);
            footer.set_visible(true);
            zoom_row.set_visible(true);
        });
    }
    {
        let config = std::env::var_os("GH_CONFIG_DIR").map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|path| path.join("gh")));
        if let Some(monitor) = config.and_then(|path| gio::File::for_path(path)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>).ok()) {
            let state = state.clone();
            monitor.connect_changed(move |_, file, other, _| {
                if [Some(file), other].into_iter().flatten().any(|file| file.basename().as_deref() == Some(std::path::Path::new("hosts.yml"))) {
                    if let Some(catalog) = state.borrow().catalog.clone() {
                        state.borrow().storage.refresh_projects(catalog, false);
                    }
                }
            });
            window.connect_close_request(move |_| { monitor.cancel(); glib::Propagation::Proceed });
        }
    }
    {
        let workspace = workspaces.clone();
        let state = state.clone();
        let window = window.clone();
        let places_scroll = places_scroll.clone();
        let search = search.clone();
        let zoom_row = zoom_row.clone();
        let footer = footer.clone();
        projects_btn.connect_clicked(move |_| {
            if let Some(catalog) = state.borrow().catalog.clone() {
                state.borrow().storage.refresh_projects(catalog, false);
            }
            if projects_page.first_child().is_none() {
                let app = window.application().expect("application window");
                projects_page.append(&project_workspace::new(&window, state.clone(), move |path| {
                    build_ui_at(&app, Some(path));
                }));
            }
            let selection = state.borrow().selection.clone();
            selection.unselect_all();
            workspace.set_visible_child_name("projects");
            places_scroll.set_visible(false);
            search.set_visible(false);
            footer.set_visible(false);
            zoom_row.set_visible(false);
        });
    }

    {
        let sel = selection.clone();
        let state = Rc::clone(&state);
        let preview_name = preview_name.clone();
        let preview_path = preview_path.clone();
        let preview_content = preview_content.clone();
        let preview_generation = Rc::new(Cell::new(0_u64));
        selection.connect_selection_changed(move |_, _, _| {
            let rows = selected_rows(&sel);
            let selected = rows.first().cloned();
            let selected_count = rows.len();
            let st = state.borrow();
            let n = st.model.n_items();
            st.manager
                .borrow_mut()
                .select(selected.as_ref().map(|row| PathBuf::from(row.path())));
            let extra = match selected_count {
                0 => String::new(),
                1 => format!(
                    "  ·  {}",
                    selected.as_ref().map(|row| row.path()).unwrap_or_default()
                ),
                count => format!("  ·  {count} selected"),
            };
            if let Some(c) = &st.catalog {
                let manager = st.manager.borrow();
                let global = manager.mode() == BrowseMode::Qfind
                    || manager.search_scope() == LocationScope::Global;
                let summary = if global {
                    format!(
                        "{n} Hits  ·  {} folders · {} files",
                        c.folder_count(),
                        c.file_count()
                    )
                } else {
                    format!(
                        "{} folders · {} files",
                        st.visible_folders, st.visible_files
                    )
                };
                st.status.set_text(&format!("{summary}{extra}"));
            }
            let generation = preview_generation.get().wrapping_add(1);
            preview_generation.set(generation);
            while let Some(child) = preview_content.first_child() {
                preview_content.remove(&child);
            }
            if selected_count > 1 {
                preview_name.set_text(&format!("{selected_count} items selected"));
                preview_path.set_text("Use Selection to rename, move or compress these items");
                return;
            }
            if let Some(row) = selected {
                preview_name.set_text(&row.name());
                preview_path.set_text(&row.path());
                let generation_now = Rc::clone(&preview_generation);
                let preview_content = preview_content.clone();
                glib::idle_add_local_once(move || {
                    if generation_now.get() != generation {
                        return;
                    }
                    let child = preview_widget(std::path::Path::new(&row.path()));
                    child.set_hexpand(true);
                    child.set_vexpand(true);
                    preview_content.append(&child);
                });
            } else {
                preview_name.set_text("Select a file");
                preview_path.set_text("Choose an item to inspect its contents and details.");
            }
        });
    }

    // Mouse buttons 8/9 (side buttons): back / forward, like every
    // file manager since forever.
    {
        let back_forward = gtk::GestureClick::new();
        back_forward.set_button(0);
        let state = Rc::clone(&state);
        back_forward.connect_pressed(move |gesture, _, _, _| match gesture.current_button() {
            8 => navigate_history(&state, true),
            9 => navigate_history(&state, false),
            _ => {}
        });
        stack.add_controller(back_forward);
    }
    install_keys(
        &window,
        &search,
        &preview_btn,
        &list,
        &selection,
        Rc::clone(&preview_slot),
        Rc::clone(&hovered),
        Rc::clone(&preview_mode),
        Rc::clone(&state),
        popover.clone(),
    );

    window.present();
    // Proof of GPU compositing for the whole shell: GSK renders every widget
    // (lists, scrolling, thumbnails, chrome) through this backend.
    if let Some(native) = window.native() {
        match native.renderer().map(|renderer| renderer.type_().name()) {
            Some(name) => eprintln!("qfind: GSK renderer: {name}"),
            None => eprintln!("qfind: GSK renderer: unavailable (software fallback)"),
        }
    }
    start_rebuild(&state, &window, false);
    start_watch(&state);
}

fn action_menu(path: &Path, actions: &mut Vec<PathBuf>) -> Option<gio::Menu> {
    let mut entries = std::fs::read_dir(path)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    let menu = gio::Menu::new();
    for path in entries {
        let label = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(['_', '-'], " ");
        if path.is_dir() {
            if let Some(child) = action_menu(&path, actions) {
                menu.append_submenu(Some(&label), &child);
            }
        } else if path.is_file() && is_executable(&path) {
            let id = actions.len();
            actions.push(path);
            menu.append(Some(&label), Some(&format!("win.external-{id}")));
        }
    }
    (menu.n_items() > 0).then_some(menu)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.extension()
            .is_some_and(|ext| ext == "exe" || ext == "bat" || ext == "cmd")
    }
}

fn hit_menu() -> (gio::Menu, Vec<PathBuf>) {
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
    let edit = gio::Menu::new();
    edit.append(Some("Rename…"), Some("win.rename"));
    edit.append(Some("Batch rename…"), Some("win.batch-rename"));
    edit.append(Some("Copy selected to…"), Some("win.batch-copy"));
    edit.append(Some("Move selected to…"), Some("win.batch-move"));
    edit.append(Some("Compress selected…"), Some("win.batch-zip"));
    edit.append(Some("Extract selected archives…"), Some("win.batch-extract"));
    edit.append(Some("New Folder…"), Some("win.mkdir"));
    menu.append_section(None, &edit);
    let remove = gio::Menu::new();
    remove.append(Some("Move to Trash"), Some("win.trash"));
    menu.append_section(None, &remove);
    let mut actions = Vec::new();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")));
    if let Some(data_home) = data_home {
        if let Some(actions_menu) = action_menu(&data_home.join("qfind/actions"), &mut actions) {
            menu.append_submenu(Some("Actions"), &actions_menu);
        }
        if let Some(scripts_menu) = action_menu(&data_home.join("nautilus/scripts"), &mut actions) {
            menu.append_submenu(Some("Nautilus Scripts"), &scripts_menu);
        }
    }
    (menu, actions)
}

/// The trio every context-menu action resolves rows through.
#[derive(Clone)]
struct ActionTarget {
    selection: gtk::MultiSelection,
    popover: gtk::PopoverMenu,
    context_target: Rc<RefCell<Option<RowData>>>,
}

impl ActionTarget {
    fn rows(&self) -> Vec<RowData> {
        action_rows(&self.selection, &self.popover, &self.context_target)
    }
}

fn install_actions(
    window: &gtk::ApplicationWindow,
    navigate: &Navigator,
    target: ActionTarget,
    preview_slot: Rc<RefCell<Option<gtk::Window>>>,
    external_actions: Vec<PathBuf>,
    state: Rc<RefCell<State>>,
) {
    manager_tools::install(window, &state, &target);
    let add = |name: &str, f: Box<dyn Fn(Vec<RowData>) + 'static>| {
        let act = gio::SimpleAction::new(name, None);
        let target = target.clone();
        act.connect_activate(move |_, _| {
            let rows = target.rows();
            if !rows.is_empty() {
                f(rows);
            }
        });
        window.add_action(&act);
    };

    let win = window.clone();
    let navigate = Rc::clone(navigate);
    add(
        "open",
        Box::new(move |rows| {
            if let Some(row) = rows.into_iter().next() {
                activate_path(&win, &navigate, row.path());
            }
        }),
    );
    let win = window.clone();
    add(
        "open-with",
        Box::new(move |rows| {
            if let Some(row) = rows.into_iter().next() {
                open_with(&win, &row.path());
            }
        }),
    );
    let win = window.clone();
    add(
        "reveal",
        Box::new(move |rows| {
            if let Some(row) = rows.into_iter().next() {
                reveal(&win, &row.path());
            }
        }),
    );
    let win = window.clone();
    add(
        "open-folder",
        Box::new(move |rows| {
            if let Some(row) = rows.into_iter().next() {
                open_folder(&win, &row.path(), row.is_dir());
            }
        }),
    );
    add(
        "copy-path",
        Box::new(|rows| {
            copy_text(
                &rows
                    .into_iter()
                    .map(|row| row.path())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }),
    );
    add(
        "copy-name",
        Box::new(|rows| {
            copy_text(
                &rows
                    .into_iter()
                    .map(|row| row.name())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }),
    );
    add(
        "copy-uri",
        Box::new(|rows| {
            copy_text(
                &rows
                    .into_iter()
                    .map(|row| gio::File::for_path(row.path()).uri())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }),
    );
    {
        let state = Rc::clone(&state);
        let window = window.clone();
        add(
            "trash",
            Box::new(move |rows| {
                trash_rows(&state, &window, rows);
            }),
        );
    }
    {
        let state = Rc::clone(&state);
        let window = window.clone();
        add(
            "rename",
            Box::new(move |rows| {
                if rows.len() > 1 {
                    manager_tools::rename(&window, &state, rows);
                } else if let Some(row) = rows.into_iter().next() {
                    rename_row(Rc::clone(&state), window.clone(), row);
                }
            }),
        );
    }
    {
        let state = Rc::clone(&state);
        let win = window.clone();
        let mkdir = gio::SimpleAction::new("mkdir", None);
        mkdir.connect_activate(move |_, _| {
            mkdir_here(Rc::clone(&state), win.clone());
        });
        window.add_action(&mkdir);
    }

    let act = gio::SimpleAction::new("preview", None);
    let win = window.clone();
    let preview_target = target.clone();
    act.connect_activate(move |_, _| {
        if let Some(row) = preview_target.rows().into_iter().next() {
            preview(win.upcast_ref(), &row.path(), &preview_slot);
        }
    });
    window.add_action(&act);

    for (id, command) in external_actions.into_iter().enumerate() {
        let act = gio::SimpleAction::new(&format!("external-{id}"), None);
        let target = target.clone();
        act.connect_activate(move |_, _| {
            let rows = target.rows();
            if rows.is_empty() {
                return;
            }
            let paths = rows.iter().map(RowData::path).collect::<Vec<_>>();
            let path = paths.first().cloned().unwrap_or_default();
            let current = Path::new(&path)
                .parent()
                .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
            let _ = Command::new(&command)
                .args(&paths)
                .env("QFIND_SELECTED_PATHS", paths.join("\n"))
                .env("QFIND_CURRENT_DIRECTORY", &current)
                .env(
                    "NAUTILUS_SCRIPT_SELECTED_FILE_PATHS",
                    format!("{}\n", paths.join("\n")),
                )
                .env(
                    "NAUTILUS_SCRIPT_SELECTED_URIS",
                    paths
                        .iter()
                        .map(|path| format!("{}\n", gio::File::for_path(path).uri()))
                        .collect::<String>(),
                )
                .env(
                    "NAUTILUS_SCRIPT_CURRENT_URI",
                    gio::File::for_path(&current).uri(),
                )
                .spawn();
        });
        window.add_action(&act);
    }
}

fn action_rows(
    selection: &impl IsA<gtk::SelectionModel>,
    popover: &gtk::PopoverMenu,
    context_target: &RefCell<Option<RowData>>,
) -> Vec<RowData> {
    let target = context_target.borrow_mut().take();
    if popover
        .parent()
        .is_some_and(|parent| parent.widget_name() == "qfind-storage-map")
    {
        target.into_iter().collect()
    } else {
        selected_rows(selection)
    }
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
    preview_btn: &gtk::ToggleButton,
    list: &gtk::ColumnView,
    selection: &gtk::MultiSelection,
    preview_slot: Rc<RefCell<Option<gtk::Window>>>,
    hovered: Rc<RefCell<Option<String>>>,
    preview_mode: Rc<Cell<qfind_core::PreviewMode>>,
    state: Rc<RefCell<State>>,
    popover: gtk::PopoverMenu,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let search = search.clone();
    let preview_btn = preview_btn.clone();
    let list = list.clone();
    let selection = selection.clone();
    let host = window.clone();
    let window = window.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        if state.borrow().host.as_ref().is_some_and(|host| !host.stack.is_mapped()) { return glib::Propagation::Proceed; }
        let search_focus = focus_in(&window, &search);
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);

        if alt && key == gdk::Key::Left {
            navigate_history(&state, true);
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Right {
            navigate_history(&state, false);
            return glib::Propagation::Stop;
        }
        if alt && key == gdk::Key::Up {
            navigate_parent(&state);
            return glib::Propagation::Stop;
        }

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
            refresh_current(&state, &window);
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::F3 {
            preview_btn.set_active(!preview_btn.is_active());
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::Down && search_focus {
            list.grab_focus();
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::f && ctrl {
            let next = {
                let st = state.borrow();
                st.manager
                    .borrow()
                    .directory()
                    .and_then(|path| st.catalog.as_ref()?.folder(path))
            };
            if shift {
                state
                    .borrow()
                    .manager
                    .borrow_mut()
                    .set_search_scope(LocationScope::Global);
            } else {
                let mut st = state.borrow_mut();
                let scope = if st.manager.borrow().directory().is_some() {
                    LocationScope::Directory
                } else {
                    LocationScope::Global
                };
                st.manager.borrow_mut().set_search_scope(scope);
                st.folder = next;
            }
            let st = state.borrow();
            let manager = st.manager.borrow();
            let placeholder = if manager.search_scope() == LocationScope::Directory {
                manager
                    .directory()
                    .map(|path| format!("Search in {}…", path.display()))
                    .unwrap_or_else(|| "Search files…".into())
            } else {
                "Search everywhere…".into()
            };
            drop(manager);
            drop(st);
            search.set_placeholder_text(Some(&placeholder));
            kick_search(&state);
            search.grab_focus();
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::s && ctrl && state.borrow().archive_save_btn.is_visible() {
            state.borrow().archive_save_btn.emit_clicked();
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::l && ctrl {
            goto_dialog(Rc::clone(&state), window.clone());
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::a && ctrl && !search_focus {
            selection.select_all();
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::k && ctrl {
            search.grab_focus();
            return glib::Propagation::Stop;
        }

        if ctrl && (key == gdk::Key::plus || key == gdk::Key::equal) {
            if let Some(host) = state.borrow().host.as_ref() {
                host.zoom.set(host.zoom.get().bump(1));
                host.apply();
            }
            return glib::Propagation::Stop;
        }
        if ctrl && key == gdk::Key::minus {
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
            let rows = selected_rows(&selection);
            if shift && rows.len() == 1 {
                copy_name(&rows[0].name());
            } else if !rows.is_empty() {
                copy_paths(&rows.into_iter().map(|row| row.path()).collect::<Vec<_>>());
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::Delete && !search_focus {
            trash_rows(&state, &window, selected_rows(&selection));
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::F2 && !search_focus {
            let rows = selected_rows(&selection);
            if rows.len() > 1 {
                manager_tools::rename(&window, &state, rows);
            } else if let Some(row) = rows.into_iter().next() {
                rename_row(Rc::clone(&state), window.clone(), row);
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::F7 && !search_focus {
            mkdir_here(Rc::clone(&state), window.clone());
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::z && ctrl && !search_focus {
            undo_last(&state, &window);
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

fn crumb_sep(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label
}

/// Rebuild the centered breadcrumb route from the manager's directory.
/// Shows the last five segments so deep paths stay centered.
fn refresh_crumbs(state: &Rc<RefCell<State>>) {
    let (crumbs, dir) = {
        let st = state.borrow();
        (
            st.crumbs.clone(),
            st.manager.borrow().directory().map(Path::to_path_buf),
        )
    };
    while let Some(child) = crumbs.first_child() {
        crumbs.remove(&child);
    }
    let Some(dir) = dir else {
        crumbs.append(&crumb_sep("Everywhere"));
        return;
    };
    let mut prefix = PathBuf::new();
    let mut segs: Vec<(String, PathBuf)> = Vec::new();
    for component in dir.components() {
        match component {
            std::path::Component::Prefix(part) => prefix.push(part.as_os_str()),
            std::path::Component::RootDir => {
                prefix.push("/");
                segs.push(("/".into(), prefix.clone()));
            }
            std::path::Component::Normal(part) => {
                prefix.push(part);
                segs.push((part.to_string_lossy().into_owned(), prefix.clone()));
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                prefix.pop();
            }
        }
    }
    let show_from = segs.len().saturating_sub(3);
    if show_from > 0 {
        crumbs.append(&crumb_sep("…"));
        crumbs.append(&crumb_sep("›"));
    }
    for (index, (name, path)) in segs.iter().enumerate().skip(show_from) {
        if index > show_from {
            crumbs.append(&crumb_sep("›"));
        }
        let button = gtk::Button::with_label(name);
        if let Some(label) = button.child().and_downcast::<gtk::Label>() {
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_max_width_chars(14);
            label.set_width_chars(if index + 1 == segs.len() { 8 } else { 4 });
        }
        button.add_css_class("flat");
        button.set_tooltip_text(Some(&path.display().to_string()));
        let state = Rc::clone(state);
        let path = path.clone();
        button.connect_clicked(move |_| navigate_to(&state, path.clone(), true));
        crumbs.append(&button);
    }
}

fn navigate_to(state: &Rc<RefCell<State>>, path: PathBuf, remember: bool) {
    let Ok(path) = path.canonicalize() else {
        state.borrow().status.set_text("Location does not exist");
        return;
    };
    if !path.is_dir() {
        state.borrow().status.set_text("Location is not a folder");
        return;
    }

    let changed = {
        let st = state.borrow();
        let mut manager = st.manager.borrow_mut();
        let changed = !remember
            || manager.directory() != Some(path.as_path())
            || manager.search_scope() != LocationScope::Directory;
        if remember {
            manager.navigate(path.clone());
        }
        changed
    };
    if !changed {
        return;
    }
    let mut st = state.borrow_mut();
    st.folder = st
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.folder(&path));
    st.search
        .set_placeholder_text(Some(&format!("Search in {}…", path.display())));
    let manager = st.manager.borrow();
    st.back_btn.set_sensitive(manager.can_back());
    st.forward_btn.set_sensitive(manager.can_forward());
    st.up_btn.set_sensitive(path.parent().is_some());
    let bookmarked = bookmarks().contains(&path);
    let bookmark_btn = st.bookmark_btn.clone();
    update_archive_save_button(&st.archive_save_btn, &path);
    if manager.mode() == BrowseMode::Qfind && st.folder.is_none() && st.catalog.is_some() {
        st.status
            .set_text("Folder is outside the Catalog · press F5 to refresh");
    }
    drop(manager);
    drop(st);
    refresh_crumbs(state);
    bookmark_btn.set_icon_name(if bookmarked {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
    state.borrow().storage.set_directory(&path);
    search_now(state);
}

fn update_archive_save_button(button: &gtk::Button, path: &Path) {
    let workspace = archive::workspace(path);
    button.set_visible(
        workspace
            .as_ref()
            .is_some_and(|workspace| archive::can_repack(&workspace.source)),
    );
    button.set_tooltip_text(
        workspace
            .map(|workspace| {
                format!(
                    "Save extracted changes back to {} (Ctrl+S)",
                    workspace.source.display()
                )
            })
            .as_deref(),
    );
}

fn navigate_history(state: &Rc<RefCell<State>>, backwards: bool) {
    let target = if backwards {
        state.borrow().manager.borrow_mut().back()
    } else {
        state.borrow().manager.borrow_mut().forward()
    };
    let Some(target) = target else { return };
    let Ok(target) = target.canonicalize() else {
        state
            .borrow()
            .status
            .set_text("Previous location no longer exists");
        return;
    };
    navigate_to(state, target, false);
}

fn navigate_parent(state: &Rc<RefCell<State>>) {
    let parent = state.borrow().manager.borrow().parent();
    if let Some(parent) = parent {
        navigate_to(state, parent, true);
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

fn search_now(state: &Rc<RefCell<State>>) {
    let seq = {
        let mut st = state.borrow_mut();
        st.seq += 1;
        st.seq
    };
    spawn_search(state, seq);
}


fn spawn_search(state: &Rc<RefCell<State>>, seq: u64) {
    let st = state.borrow();
    let catalog = st.catalog.clone();
    let q = st.search.text().to_string();
    let opts = opts_from(&st);
    let folder = st.folder.clone();
    let manager = st.manager.borrow();
    let folder_path = manager.directory().map(Path::to_path_buf);
    let folder_scope = manager.search_scope() == LocationScope::Directory;
    let recursive = manager.mode() == BrowseMode::Qfind;
    drop(manager);
    let measure_size = st.host.as_ref().is_some_and(|host| host.show_weight.get());
    let folders_first = FOLDERS_FIRST.load(AtomicOrdering::Relaxed);
    drop(st);
    let state = Rc::clone(state);
    glib::MainContext::default().spawn_local(async move {
        let result = gio::spawn_blocking(move || match (folder_scope, recursive) {
            (true, false) => folder_path
                .as_deref()
                .ok_or_else(|| "No folder selected".to_owned())
                .and_then(|path| live_children(path, &q, opts, folders_first, measure_size).map_err(|error| error.to_string()))
                .map(SearchResult::Live),
            (true, true) => folder
                .ok_or_else(|| {
                    "Folder is outside the Catalog; refresh the Catalog for Qfind mode".to_owned()
                })
                .and_then(|folder| {
                    folder
                        .search_with(&q, opts)
                        .map(|hits| hits.ids().to_vec())
                        .map_err(|error| error.to_string())
                })
                .map(SearchResult::Indexed),
            (false, _) => catalog
                .ok_or_else(|| "Catalog is not ready".to_owned())
                .and_then(|catalog| {
                    catalog
                        .search_with(&q, opts)
                        .map(|hits| hits.ids().to_vec())
                        .map_err(|error| error.to_string())
                })
                .map(SearchResult::Indexed),
        })
        .await;
        if state.borrow().seq != seq {
            return;
        }
        match result {
            Ok(result) => match result {
                Ok(SearchResult::Indexed(mut ids)) => {
                    let (host, catalog, selected_id, chart_dir) = {
                        let st = state.borrow();
                        let manager = st.manager.borrow();
                        // Honor the Directory/Global toggle: Directory roots
                        // the chart at the browsed folder, Global shows all.
                        let scoped = manager.chart_scope() == ChartScope::Directory;
                        (
                            st.host.clone(),
                            st.catalog.clone(),
                            (!st.selection.selection().is_empty())
                                .then(|| st.model.id(st.selection.selection().nth(0)))
                                .flatten(),
                            scoped
                                .then(|| manager.directory().map(Path::to_path_buf))
                                .flatten(),
                        )
                    };
                    if FOLDERS_FIRST.load(AtomicOrdering::Relaxed) {
                        if let Some(catalog) = &catalog {
                            ids.sort_by_key(|&id| !catalog.hit(id).is_some_and(|hit| hit.is_dir()));
                        }
                    }
                    let n = ids.len();
                    let (folders, files) = catalog
                        .as_ref()
                        .map(|catalog| {
                            let folders = ids
                                .iter()
                                .filter(|&&id| catalog.hit(id).is_some_and(|hit| hit.is_dir()))
                                .count();
                            (folders, ids.len().saturating_sub(folders))
                        })
                        .unwrap_or_default();
                    {
                        let mut st = state.borrow_mut();
                        st.last_ids = ids.clone();
                        st.visible_folders = folders;
                        st.visible_files = files;
                    }
                    state.borrow().selection.unselect_all();
                    state.borrow().model.set_ids(ids.clone());
                    if let Some(position) =
                        selected_id.and_then(|id| state.borrow().model.position(id))
                    {
                        state.borrow().selection.select_item(position, true);
                    }
                    if let (Some(host), Some(c)) = (host, catalog) {
                        if host.surface.get() == Surface::Tree {
                            surface::rebuild_tree(&host, &c, &ids);
                        }
                        if host.show_weight.get() {
                            surface::rebuild_weight(&host, &c, &ids, chart_dir.as_deref());
                        }
                        host.apply();
                    }
                    let st = state.borrow();
                    if let Some(c) = &st.catalog {
                        let manager = st.manager.borrow();
                        let summary = if manager.mode() == BrowseMode::Qfind
                            || manager.search_scope() == LocationScope::Global
                        {
                            format!(
                                "{n} Hits  ·  {} folders · {} files",
                                c.folder_count(),
                                c.file_count()
                            )
                        } else {
                            format!("{folders} folders · {files} files")
                        };
                        st.status.set_text(&summary);
                    }
                }
                Ok(SearchResult::Live(mut rows)) => {
                    let sort = state.borrow().sort;
                    if matches!(sort, Sort::Largest | Sort::Smallest) {
                        let storage = state.borrow().storage.clone();
                        let folders_first = FOLDERS_FIRST.load(AtomicOrdering::Relaxed);
                        rows.sort_by_cached_key(|row| {
                            let bytes = if row.is_dir { storage.known_size(&row.path) } else { Some(row.size) };
                            (folders_first && !row.is_dir, bytes.is_none(), if sort == Sort::Largest { u64::MAX - bytes.unwrap_or(0) } else { bytes.unwrap_or(0) }, row.sort_name.clone())
                        });
                    }
                    let (folders, files) = rows.iter().fold((0, 0), |(folders, files), row| {
                        if row.is_dir {
                            (folders + 1, files)
                        } else {
                            (folders, files + 1)
                        }
                    });
                    let show_weight = state
                        .borrow()
                        .host
                        .as_ref()
                        .is_some_and(|host| host.show_weight.get());
                    let weights = show_weight.then(|| {
                        rows.iter()
                            .map(|row| qfind_core::Weighted {
                                name: row.name.clone(),
                                path: row.path.to_string_lossy().into_owned(),
                                weight: row.size.max(1),
                                id: None,
                            })
                            .collect()
                    });
                    let selected_path =
                        selected_row(&state.borrow().selection).map(|row| row.path());
                    let model_rows = rows
                        .into_iter()
                        .map(|row| {
                            RowData::new(
                                row.name,
                                row.path.to_string_lossy(),
                                row.is_dir,
                                row.size,
                                row.mtime,
                            )
                        })
                        .collect();
                    let (model, status, host) = {
                        let mut st = state.borrow_mut();
                        st.last_ids.clear();
                        st.visible_folders = folders;
                        st.visible_files = files;
                        (st.model.clone(), st.status.clone(), st.host.clone())
                    };
                    // GTK model changes notify selection synchronously. Never emit them
                    // while State is borrowed: its selection callback reads State again.
                    state.borrow().selection.unselect_all();
                    model.set_rows(model_rows);
                    if let Some(host) = host {
                        if let Some(weights) = weights {
                            surface::rebuild_weight_values(&host, weights);
                        }
                        host.apply();
                    }
                    status.set_text(&format!("{folders} folders · {files} files"));
                    if let Some(position) = selected_path
                        .as_deref()
                        .and_then(|path| state.borrow().model.position_path(path))
                    {
                        state.borrow().selection.select_item(position, true);
                    }
                }
                Err(err) => state.borrow().status.set_text(&err),
            },
            Err(_) => state.borrow().status.set_text("Search worker failed"),
        }
    });
}

fn adopt_catalog(state: &Rc<RefCell<State>>, catalog: Catalog) {
    let mtime = std::fs::metadata(catalog.path())
        .and_then(|m| m.modified())
        .ok();
    state.borrow().model.set_catalog(catalog.clone());
    let directory = state
        .borrow()
        .manager
        .borrow()
        .directory()
        .map(Path::to_path_buf);
    {
        let mut st = state.borrow_mut();
        st.snap_mtime = mtime;
        st.folder = directory.as_deref().and_then(|path| catalog.folder(path));
        st.status.set_text(&format!(
            "Catalog ready  ·  {} folders · {} files",
            catalog.folder_count(),
            catalog.file_count()
        ));
        st.catalog = Some(catalog.clone());
    }
    state.borrow().storage.set_catalog(catalog.clone());
    let warm = catalog;
    thread::spawn(move || warm.warm());
    search_now(state);
}

fn refresh_current(state: &Rc<RefCell<State>>, window: &gtk::ApplicationWindow) {
    if state.borrow().manager.borrow().mode() == BrowseMode::Qfind {
        start_rebuild(state, window, true);
    } else {
        state.borrow().status.set_text("Refreshing folder…");
        kick_search(state);
    }
}

fn push_undo(state: &Rc<RefCell<State>>, entry: UndoEntry) {
    let mut st = state.borrow_mut();
    if st.undo.len() >= MAX_UNDO {
        st.undo.remove(0);
    }
    st.undo.push(entry);
}

/// Trash rows through the core ops (undoable, errors surfaced) and refresh.
fn trash_rows(state: &Rc<RefCell<State>>, window: &gtk::ApplicationWindow, rows: Vec<RowData>) {
    if rows.is_empty() {
        return;
    }
    let mut trashed = 0usize;
    let mut failed: Option<String> = None;
    for row in rows {
        let path = PathBuf::from(row.path());
        match qfind_core::trash(&path) {
            Ok((staged, _)) => {
                trashed += 1;
                push_undo(state, UndoEntry::Trash { staged, orig: path });
            }
            Err(error) => {
                failed = Some(error.to_string());
                break;
            }
        }
    }
    {
        let st = state.borrow();
        st.status.set_text(&match failed {
            Some(error) => format!("Trashed {trashed}, stopped: {error}"),
            None => format!("Trashed {trashed} (Ctrl+Z undo)"),
        });
    }
    refresh_current(state, window);
}

fn undo_last(state: &Rc<RefCell<State>>, window: &gtk::ApplicationWindow) {
    let entry = state.borrow_mut().undo.pop();
    let Some(entry) = entry else {
        state.borrow().status.set_text("Nothing to undo");
        return;
    };
    let message = match entry.undo() {
        Ok(message) => message,
        Err(error) => format!("Undo failed: {error}"),
    };
    state.borrow().status.set_text(&message);
    refresh_current(state, window);
}

fn current_dir(state: &Rc<RefCell<State>>) -> PathBuf {
    state
        .borrow()
        .manager
        .borrow()
        .directory()
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Modal one-line text prompt. `accept` runs with the trimmed input.
fn prompt_text(
    window: &gtk::ApplicationWindow,
    title: &str,
    initial: &str,
    accept: impl Fn(String) + 'static,
) {
    let dialog = gtk::Window::builder()
        .title(title)
        .modal(true)
        .transient_for(window)
        .default_width(420)
        .build();
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_margin_top(12);
    entry.set_margin_start(12);
    entry.set_margin_end(12);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(12);
    buttons.set_margin_bottom(12);
    buttons.set_margin_start(12);
    buttons.set_margin_end(12);
    buttons.append(&cancel);
    buttons.append(&apply);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&entry);
    vbox.append(&buttons);
    dialog.set_child(Some(&vbox));
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let entry = entry.clone();
        apply.connect_clicked(move |_| {
            accept(entry.text().trim().to_string());
            dialog.close();
        });
    }
    entry.connect_activate({
        let apply = apply.clone();
        move |_| apply.emit_clicked()
    });
    let esc = gtk::EventControllerKey::new();
    {
        let dialog = dialog.clone();
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                dialog.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    dialog.add_controller(esc);
    dialog.present();
    entry.grab_focus();
    entry.select_region(-1, -1);
}

/// Ctrl+L "Go to folder": same resolve rules the old location entry had
/// (absolute, or relative to the browsed directory / cwd).
fn goto_dialog(state: Rc<RefCell<State>>, window: gtk::ApplicationWindow) {
    let initial = current_dir(&state).display().to_string();
    prompt_text(&window, "Go to folder", &initial, move |raw| {
        let path = PathBuf::from(&raw);
        let path = if path.is_absolute() {
            path
        } else {
            current_dir(&state).join(path)
        };
        navigate_to(&state, path, true);
    });
}

fn rename_row(state: Rc<RefCell<State>>, window: gtk::ApplicationWindow, row: RowData) {
    let from = PathBuf::from(row.path());
    let initial = from
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let win = window.clone();
    prompt_text(&window, "Rename", &initial, move |name| {
        if name.is_empty() || name.contains('/') {
            state.borrow().status.set_text("Name must not be empty");
            return;
        }
        let Some(parent) = from.parent() else {
            state
                .borrow()
                .status
                .set_text("Cannot rename the filesystem root");
            return;
        };
        let to = parent.join(&name);
        if to == from {
            return;
        }
        match qfind_core::rename(&from, &to) {
            Ok(_) => {
                push_undo(
                    &state,
                    UndoEntry::Rename {
                        from: from.clone(),
                        to: to.clone(),
                    },
                );
                state
                    .borrow()
                    .status
                    .set_text(&format!("Renamed to {} (Ctrl+Z undo)", to.display()));
                refresh_current(&state, &win);
            }
            Err(error) => state
                .borrow()
                .status
                .set_text(&format!("Rename failed: {error}")),
        }
    });
}

fn mkdir_here(state: Rc<RefCell<State>>, window: gtk::ApplicationWindow) {
    let parent = current_dir(&state);
    let win = window.clone();
    prompt_text(&window, "New folder", "", move |name| {
        if name.is_empty() || name.contains('/') {
            state.borrow().status.set_text("Name must not be empty");
            return;
        }
        let path = parent.join(&name);
        match qfind_core::create_dir(&path) {
            Ok(_) => {
                push_undo(&state, UndoEntry::Created { path: path.clone() });
                state
                    .borrow()
                    .status
                    .set_text(&format!("Created {} (Ctrl+Z undo)", path.display()));
                refresh_current(&state, &win);
            }
            Err(error) => state
                .borrow()
                .status
                .set_text(&format!("Cannot create folder: {error}")),
        }
    });
}

fn start_rebuild(state: &Rc<RefCell<State>>, _window: &gtk::ApplicationWindow, force: bool) {
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
    let state = Rc::clone(state);
    glib::MainContext::default().spawn_local(async move {
        match gio::spawn_blocking(move || Catalog::rebuild(Config::load().rebuild())).await {
            Ok(Ok(catalog)) => adopt_catalog(&state, catalog),
            Ok(Err(err)) => state
                .borrow()
                .status
                .set_text(&format!("rebuild failed: {err}")),
            Err(_) => state.borrow().status.set_text("Catalog rebuild failed"),
        }
    });
}

fn start_watch(state: &Rc<RefCell<State>>) {
    let path = default_snapshot_path();
    let Some(parent) = path.parent() else { return };
    let _ = std::fs::create_dir_all(parent);
    let Ok(monitor) = gio::File::for_path(parent)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    else {
        return;
    };
    let state = Rc::clone(state);
    let pending = Rc::new(Cell::new(false));
    let keep_alive = monitor.clone();
    monitor.connect_changed(move |_, file, other, event| {
        let _ = &keep_alive;
        if !matches!(
            event,
            gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::MovedIn
                | gio::FileMonitorEvent::Renamed
        ) || (file.path().as_deref() != Some(path.as_path())
            && other.and_then(gio::File::path).as_deref() != Some(path.as_path()))
            || pending.replace(true)
        {
            return;
        }
        let state = Rc::clone(&state);
        let pending = Rc::clone(&pending);
        let path = path.clone();
        glib::idle_add_local_once(move || {
            pending.set(false);
            let Ok(meta) = std::fs::metadata(&path) else {
                return;
            };
            let Ok(mtime) = meta.modified() else { return };
            if state.borrow().snap_mtime == Some(mtime) {
                return;
            }
            if let Ok(catalog) = Catalog::open(&path) {
                adopt_catalog(&state, catalog);
                state
                    .borrow()
                    .status
                    .set_text("Catalog reloaded (snapshot changed)");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Instant;

    use tempfile::tempdir;

    use super::*;

    fn drain_events() {
        let context = glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
    }

    fn wait_for(predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() && Instant::now() < deadline {
            drain_events();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(predicate(), "GTK model did not settle before the deadline");
    }

    #[test]
    fn manager_mode_and_preview_controls_follow_clicks() {
        gtk::init().unwrap();

        let classic = gtk::ToggleButton::with_label("Classic");
        classic.set_active(true);
        let qfind = gtk::ToggleButton::with_label("Qfind");
        qfind.set_group(Some(&classic));
        let recursive = Rc::new(Cell::new(false));
        let recursive_live = Rc::clone(&recursive);
        bind_mode_buttons(&classic, &qfind, move |value| recursive_live.set(value));

        qfind.emit_clicked();
        drain_events();
        assert!(qfind.is_active());
        assert!(recursive.get());

        classic.emit_clicked();
        drain_events();
        assert!(classic.is_active());
        assert!(!recursive.get());

        let toggle = gtk::ToggleButton::new();
        toggle.set_active(true);
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let close = gtk::Button::new();
        bind_preview_controls(&toggle, &pane, &close);

        close.emit_clicked();
        drain_events();
        assert!(!toggle.is_active());
        assert!(!pane.is_visible());

        toggle.emit_clicked();
        drain_events();
        assert!(toggle.is_active());
        assert!(pane.is_visible());

        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("folder")).unwrap();
        fs::write(root.join("file.txt"), b"file").unwrap();
        fs::write(root.join("folder/nested.txt"), b"nested").unwrap();
        let catalog = Catalog::rebuild(
            qfind_core::Rebuild::new(temp.path().join("catalog")).roots([root.clone()]),
        )
        .unwrap();
        let model = HitModel::new();
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let search = gtk::SearchEntry::new();
        let manager = Rc::new(RefCell::new(ManagerSession::new(Some(root.clone()))));
        let state = Rc::new(RefCell::new(State {
            catalog: None,
            folder: None,
            manager: Rc::clone(&manager),
            crumbs: gtk::Box::new(gtk::Orientation::Horizontal, 2),
            back_btn: gtk::Button::new(),
            forward_btn: gtk::Button::new(),
            up_btn: gtk::Button::new(),
            bookmark_btn: gtk::Button::new(),
            archive_save_btn: gtk::Button::new(),
            model: model.clone(),
            selection,
            status: gtk::Label::new(None),
            search,
            scope: Scope::All,
            class: FileClass::All,
            sort: Sort::Score,
            match_mode: MatchMode::Fuzzy,
            seq: 1,
            snap_mtime: None,
            last_ids: Vec::new(),
            visible_folders: 0,
            visible_files: 0,
            host: None,
            storage: storage::Pane::new(manager),
            undo: Vec::new(),
        }));

        spawn_search(&state, 1);
        wait_for(|| model.n_items() == 2);
        assert_eq!(state.borrow().visible_folders, 1);
        assert_eq!(state.borrow().visible_files, 1);

        let selection = state.borrow().selection.clone();
        let selection_reads = Rc::new(Cell::new(0));
        let reads = Rc::clone(&selection_reads);
        let state_on_selection = Rc::clone(&state);
        selection.connect_selection_changed(move |_, _, _| {
            let _state = state_on_selection.borrow();
            reads.set(reads.get() + 1);
        });
        selection.select_item(1, true);
        selection.select_item(0, false);
        assert_eq!(selected_rows(&selection).len(), 2);
        drain_events();
        let before_refresh = selection_reads.get();
        state.borrow().search.set_text("folder");
        state.borrow_mut().seq = 2;
        spawn_search(&state, 2);
        wait_for(|| selection_reads.get() > before_refresh);

        {
            let mut state = state.borrow_mut();
            state.search.set_text("");
            state.model.set_catalog(catalog.clone());
            state.folder = catalog.folder(&root);
            state.catalog = Some(catalog);
            state.manager.borrow_mut().set_mode(BrowseMode::Qfind);
            state.seq = 3;
        }
        spawn_search(&state, 3);
        wait_for(|| model.n_items() == 3);
    }

    #[test]
    fn live_directory_hot_path_stays_subsecond_at_row_limit() {
        let temp = tempdir().unwrap();
        for index in (0..MAX_ROWS).rev() {
            fs::write(temp.path().join(format!("MixedCase-{index:04}.txt")), []).unwrap();
        }
        let started = Instant::now();
        let rows = live_children(
            temp.path(),
            "",
            SearchOpts {
                scope: Scope::All,
                class: FileClass::All,
                sort: Sort::Name,
                date: DateAge::Any,
                limit: MAX_ROWS,
                highlight: false,
                match_mode: MatchMode::Fuzzy,
            },
            true,
            false,
        )
        .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(rows.len(), MAX_ROWS);
        assert_eq!(rows.first().unwrap().name, "MixedCase-0000.txt");
        assert!(elapsed < Duration::from_secs(1), "scan took {elapsed:?}");
        eprintln!("5,000-row live scan and sort: {elapsed:?}");
    }
}
