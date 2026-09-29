//! Settings window: Catalog, PreviewMode, and opening behavior.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use qfind_core::appearance::{
    APPEARANCES, GTK_THEMES, THEMES, accent_for, normalize_appearance, normalize_gtk_theme,
    normalize_theme,
};
use qfind_core::{Config, MatchMode, OpenMode, PreviewMode};

thread_local! {
    static SYSTEM_GTK_THEME: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
    static SYSTEM_ICON_THEME: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
    /// One Settings window at a time. Each used to carry its own widget state
    /// and its own `on_save`, so saving in one could Rebuild the Catalog out
    /// from under another that was still open and stale.
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

/// `native`: leave the toolkit theme alone, take its accent.
pub fn is_native(cfg: &Config) -> bool {
    normalize_appearance(&cfg.appearance) == "native"
}

/// Install the stylesheet, icons, and GTK theme for `cfg`. Cheap; call on save.
///
/// The whole sheet is (re)loaded every time, so switching Appearance between
/// `native` and `custom` takes effect immediately instead of needing a restart.
/// It used to be assembled once at startup, so the change was written to disk
/// while the app kept looking the other way — and the header logo, which uses
/// our bundled icon, rendered as a broken image.
pub fn apply_appearance(cfg: &Config) {
    let native = is_native(cfg);
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let css = format!(
        "{}\n@define-color qfind_accent {};",
        sheet_for(cfg),
        if native {
            "@accent_bg_color".to_owned()
        } else {
            accent_for(&cfg.theme).to_owned()
        }
    );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    // Idempotent: GTK ignores re-adding the same provider.
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    if native {
        // `icons::install` set ours process-wide, GTK's own file chooser
        // included. Switching back has to put the user's theme back.
        if let Some(settings) = gtk::Settings::default() {
            let system_icons = SYSTEM_ICON_THEME.with(|slot| {
                slot.borrow_mut()
                    .get_or_insert_with(|| {
                        settings.gtk_icon_theme_name().map(|name| name.to_string())
                    })
                    .clone()
            });
            if let Some(system_icons) = system_icons {
                settings.set_gtk_icon_theme_name(Some(&system_icons));
            }
        }
    } else {
        crate::icons::install();
    }
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    let system = SYSTEM_GTK_THEME.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| settings.gtk_theme_name().map(|name| name.to_string()))
            .clone()
    });
    let wanted = if native {
        "system"
    } else {
        normalize_gtk_theme(&cfg.gtk_theme)
    };
    match wanted {
        "system" => settings.set_gtk_theme_name(system.as_deref()),
        name => settings.set_gtk_theme_name(Some(name)),
    }
}

/// The stylesheet body for the current Appearance: the toolkit-neutral part of
/// `design.css`, plus the user's `custom.css` when in `custom` mode.
fn sheet_for(cfg: &Config) -> String {
    const DESIGN: &str = include_str!("design.css");
    const CUSTOM_MARKER: &str = "/* --- custom palette ---";
    let base = match (is_native(cfg), DESIGN.split_once(CUSTOM_MARKER)) {
        (true, Some((before, _))) => before,
        _ => DESIGN,
    };
    if is_native(cfg) {
        return base.to_owned();
    }
    // User overrides live next to config.toml, so they win.
    match Config::path().parent().map(|dir| dir.join("custom.css")) {
        Some(user) if user.is_file() => match std::fs::read_to_string(&user) {
            Ok(extra) => format!("{base}\n{extra}"),
            Err(error) => {
                gtk::glib::g_warning!("megaman", "could not read {}: {error}", user.display());
                base.to_owned()
            }
        },
        _ => base.to_owned(),
    }
}

fn index_of(list: &[&str], wanted: &str) -> u32 {
    list.iter().position(|t| *t == wanted).unwrap_or(0) as u32
}

pub struct Live {
    pub preview: Rc<Cell<PreviewMode>>,
    pub zebra: Rc<Cell<bool>>,
    pub weight: Rc<Cell<bool>>,
    pub match_mode: Rc<Cell<MatchMode>>,
    pub on_save: Box<dyn Fn(bool)>,
}

/// A short modal error. Every failure the user can act on goes through here.
fn alert(parent: &gtk::Window, message: &str, detail: &str) {
    gtk::AlertDialog::builder()
        .modal(true)
        .message(message)
        .detail(detail)
        .build()
        .show(Some(parent));
}

/// Open Settings. A second request focuses nothing new: each window used to
/// carry its own widget state and its own `on_save`, so saving in one could
/// Rebuild the Catalog while another was still open and stale.
pub fn open(parent: &gtk::ApplicationWindow, live: Live) {
    if OPEN.get() {
        return;
    }
    OPEN.set(true);
    let cfg = Config::load();
    let win = gtk::Window::builder()
        .transient_for(parent)
        .title("Megaman Settings")
        .default_width(560)
        .default_height(640)
        .modal(true)
        .build();
    let header = gtk::HeaderBar::new();
    header.set_show_title_buttons(true);
    win.set_titlebar(Some(&header));

    let keys = gtk::EventControllerKey::new();
    {
        let win = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                win.close();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
    }
    win.add_controller(keys);

    let exclude = list_editor(
        "Exclude",
        "Names or globs skipped on the next Rebuild.",
        &cfg.exclude,
    );
    let include = list_editor(
        "Mounts",
        "Roots to index. Empty discovers every local disk.",
        &cfg.include
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
    );

    let preview_drop =
        gtk::DropDown::from_strings(&["Hovered Hit (Space)", "Selected Hit (Space)"]);
    preview_drop.set_selected(match cfg.preview {
        PreviewMode::Hovered => 0,
        PreviewMode::Selected => 1,
    });
    let match_drop = gtk::DropDown::from_strings(&[
        "Fuzzy (hlo → hello.txt)",
        "Substring (contiguous)",
        "Exact filename",
    ]);
    match_drop.set_tooltip_text(Some(
        "Fuzzy is on by default. Substring turns gaps off. Exact is the whole name.",
    ));
    match_drop.set_selected(match cfg.match_mode {
        MatchMode::Fuzzy => 0,
        MatchMode::Substring => 1,
        MatchMode::Exact => 2,
    });
    let open_drop = gtk::DropDown::from_strings(&[
        "Auto (EDITOR for text, desktop otherwise)",
        "Desktop handler (xdg / MIME)",
        "Editor ($EDITOR / $VISUAL)",
    ]);
    open_drop.set_tooltip_text(Some(
        "Auto uses $EDITOR or $VISUAL for source and config files. Folders and media stay with the desktop handler.",
    ));
    open_drop.set_selected(match cfg.open {
        OpenMode::Auto => 0,
        OpenMode::Xdg => 1,
        OpenMode::Editor => 2,
    });
    let editor_entry = gtk::Entry::new();
    editor_entry.set_placeholder_text(Some("$EDITOR then $VISUAL"));
    editor_entry.set_text(&cfg.editor);
    let appearance_drop = gtk::DropDown::from_strings(APPEARANCES);
    appearance_drop.set_tooltip_text(Some(
        "custom: Megaman colors, icons and ~/.config/qfind/custom.css. native: the desktop GTK/Qt theme untouched.",
    ));
    appearance_drop.set_selected(index_of(APPEARANCES, normalize_appearance(&cfg.appearance)));
    let theme_drop = gtk::DropDown::from_strings(THEMES);
    theme_drop.set_tooltip_text(Some(
        "Accent color. Shared with the TUI and every other frontend.",
    ));
    theme_drop.set_selected(index_of(THEMES, normalize_theme(&cfg.theme)));
    let gtk_theme_drop = gtk::DropDown::from_strings(GTK_THEMES);
    gtk_theme_drop.set_tooltip_text(Some(
        "system follows the desktop. Adwaita-dark forces dark.",
    ));
    gtk_theme_drop.set_selected(index_of(GTK_THEMES, normalize_gtk_theme(&cfg.gtk_theme)));

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 18);
    vbox.add_css_class("megaman-settings");
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);

    let search = group("Search");
    row(
        &search.1,
        "Query matching",
        "Fuzzy lets letters skip; Substring must be contiguous.",
        &match_drop,
    );
    row(
        &search.1,
        "Space preview",
        "Which Hit the preview follows.",
        &preview_drop,
    );
    vbox.append(&search.0);

    let opening = group("Opening");
    row(
        &opening.1,
        "Open Hits with",
        "Auto picks the editor for text and the desktop handler otherwise.",
        &open_drop,
    );
    editor_entry.set_width_chars(18);
    row(
        &opening.1,
        "Editor",
        "Empty uses $EDITOR, then $VISUAL.",
        &editor_entry,
    );
    vbox.append(&opening.0);

    let appearance = group("Appearance");
    row(
        &appearance.1,
        "Mode",
        "native follows the desktop theme; custom uses Megaman's palette and custom.css.",
        &appearance_drop,
    );
    row(
        &appearance.1,
        "Accent",
        "Shared with the TUI and every other frontend.",
        &theme_drop,
    );
    row(
        &appearance.1,
        "GTK theme",
        "system follows the desktop.",
        &gtk_theme_drop,
    );
    vbox.append(&appearance.0);

    let index = group("Index");
    exclude.root.set_margin_start(12);
    exclude.root.set_margin_end(12);
    exclude.root.set_margin_top(8);
    exclude.root.set_margin_bottom(8);
    index.1.append(&exclude.root);
    include.root.set_margin_start(12);
    include.root.set_margin_end(12);
    include.root.set_margin_top(8);
    include.root.set_margin_bottom(8);
    index.1.append(&include.root);
    vbox.append(&index.0);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let reset = gtk::Button::with_label("Reset to default");
    let save = gtk::Button::with_label("Save");
    save.set_widget_name("qfind-settings-save");
    save.add_css_class("suggested-action");
    buttons.append(&reset);
    buttons.append(&save);
    vbox.append(&buttons);

    let scroll = gtk::ScrolledWindow::builder()
        .child(&vbox)
        .hexpand(true)
        .vexpand(true)
        .build();
    win.set_child(Some(&scroll));

    let live = Rc::new(live);
    let reset_all = Rc::new(Cell::new(false));
    {
        let live = Rc::clone(&live);
        let reset_all = Rc::clone(&reset_all);
        let exclude = exclude.clone();
        let include = include.clone();
        let preview_drop = preview_drop.clone();
        let match_drop = match_drop.clone();
        let open_drop = open_drop.clone();
        let editor_entry = editor_entry.clone();
        let appearance_drop = appearance_drop.clone();
        let theme_drop = theme_drop.clone();
        let gtk_theme_drop = gtk_theme_drop.clone();
        let win = win.clone();
        save.connect_clicked(move |_| {
            let mut cfg = if reset_all.replace(false) {
                Config::default()
            } else {
                Config::load()
            };
            let old_exclude = cfg.exclude.clone();
            let old_include = cfg.include.clone();
            cfg.exclude = exclude.items();
            cfg.include = include
                .items()
                .into_iter()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
            cfg.preview = if preview_drop.selected() == 1 {
                PreviewMode::Selected
            } else {
                PreviewMode::Hovered
            };
            cfg.zebra = live.zebra.get();
            cfg.weight_map = live.weight.get();
            cfg.match_mode = match match_drop.selected() {
                1 => MatchMode::Substring,
                2 => MatchMode::Exact,
                _ => MatchMode::Fuzzy,
            };
            cfg.open = match open_drop.selected() {
                1 => OpenMode::Xdg,
                2 => OpenMode::Editor,
                _ => OpenMode::Auto,
            };
            cfg.editor = editor_entry.text().to_string();
            cfg.appearance =
                APPEARANCES[appearance_drop.selected() as usize % APPEARANCES.len()].into();
            cfg.theme = THEMES[theme_drop.selected() as usize % THEMES.len()].into();
            cfg.gtk_theme =
                GTK_THEMES[gtk_theme_drop.selected() as usize % GTK_THEMES.len()].into();
            // A read-only `$XDG_CONFIG_HOME` or a full disk used to be swallowed
            // with `let _ =`: the window closed, the live theme had already been
            // mutated, so the app *looked* like the change took and silently
            // reverted on next launch.
            if let Err(error) = cfg.save() {
                alert(
                    &win,
                    "Could not save settings",
                    &format!(
                        "{} could not be written. Your change is active for this session only.\n\n{error}",
                        Config::path().display()
                    ),
                );
                return;
            }
            apply_appearance(&cfg);
            live.preview.set(cfg.preview);
            live.match_mode.set(cfg.match_mode);
            (live.on_save)(catalog_settings_changed(&old_exclude, &old_include, &cfg));
            win.close();
        });
    }
    {
        let exclude = exclude.clone();
        let include = include.clone();
        let preview_drop = preview_drop.clone();
        let match_drop = match_drop.clone();
        let open_drop = open_drop.clone();
        let editor_entry = editor_entry.clone();
        let appearance_drop = appearance_drop.clone();
        let theme_drop = theme_drop.clone();
        let gtk_theme_drop = gtk_theme_drop.clone();
        // Reset every widget the window owns, and every setting it does not.
        // It used to leave `exclude_paths`, the visibility flags, zoom, spacing,
        // and preview width alone, and the Save handler started from
        // `Config::load()` and wrote those keys back verbatim.
        reset.connect_clicked(move |_| {
            let defaults = Config::default();
            appearance_drop.set_selected(0);
            exclude.set_items(&defaults.exclude);
            include.set_items(
                &defaults
                    .include
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
            );
            preview_drop.set_selected(0);
            match_drop.set_selected(0);
            open_drop.set_selected(0);
            editor_entry.set_text(&defaults.editor);
            theme_drop.set_selected(0);
            gtk_theme_drop.set_selected(0);
            // A full reset, not just the fields this window shows: zebra, zoom,
            // spacing, Preview width, WeightMap, the visibility flags, and
            // `exclude_paths` live in the View popover, and the Save handler
            // started from `Config::load()` and wrote them back verbatim.
            reset_all.set(true);
        });
    }

    win.connect_close_request(|win| {
        OPEN.set(false);
        let _ = win;
        gtk::glib::Propagation::Proceed
    });
    win.connect_close_request(|win| {
        OPEN.set(false);
        let _ = win;
        gtk::glib::Propagation::Proceed
    });
    win.present();
}

fn catalog_settings_changed(
    old_exclude: &[String],
    old_include: &[PathBuf],
    next: &Config,
) -> bool {
    old_exclude != next.exclude || old_include != next.include
}

fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l
}

fn hint(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.add_css_class("dim-label");
    l.add_css_class("caption");
    l
}

/// A titled card of rows, the GNOME preferences-group shape without libadwaita.
fn group(title: &str) -> (gtk::Box, gtk::ListBox) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let heading = label(title);
    heading.add_css_class("megaman-settings-group");
    root.append(&heading);
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    root.append(&list);
    (root, list)
}

/// Title + subtitle on the left, the control on the right.
fn row(list: &gtk::ListBox, title: &str, subtitle: &str, control: &impl IsA<gtk::Widget>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.set_margin_top(10);
    row.set_margin_bottom(10);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    text.append(&name);
    text.append(&hint(subtitle));
    row.append(&text);
    control.set_valign(gtk::Align::Center);
    row.append(control);
    list.append(&row);
}

#[derive(Clone)]
struct ListEdit {
    root: gtk::Box,
    rows: Rc<RefCell<gtk::Box>>,
}

impl ListEdit {
    fn items(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut child = self.rows.borrow().first_child();
        while let Some(row) = child {
            if let Some(entry) = row.first_child().and_downcast::<gtk::Entry>() {
                let t = entry.text().to_string();
                if !t.trim().is_empty() {
                    out.push(t.trim().to_string());
                }
            }
            child = row.next_sibling();
        }
        out
    }

    fn set_items(&self, items: &[String]) {
        while let Some(c) = self.rows.borrow().first_child() {
            self.rows.borrow().remove(&c);
        }
        if items.is_empty() {
            self.rows.borrow().append(&entry_row(""));
        } else {
            for i in items {
                self.rows.borrow().append(&entry_row(i));
            }
        }
    }
}

fn list_editor(title: &str, subtitle: &str, items: &[String]) -> ListEdit {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    root.append(&name);
    root.append(&hint(subtitle));
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 4);
    if items.is_empty() {
        rows.append(&entry_row(""));
    } else {
        for i in items {
            rows.append(&entry_row(i));
        }
    }
    let rows = Rc::new(RefCell::new(rows));
    root.append(&*rows.borrow());
    let add = gtk::Button::with_label("Add");
    add.set_halign(gtk::Align::Start);
    add.add_css_class("flat");
    {
        let rows = Rc::clone(&rows);
        add.connect_clicked(move |_| {
            rows.borrow().append(&entry_row(""));
        });
    }
    root.append(&add);
    ListEdit { root, rows }
}

fn entry_row(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let entry = gtk::Entry::new();
    entry.set_text(text);
    entry.set_hexpand(true);
    let rm = gtk::Button::from_icon_name("list-remove-symbolic");
    rm.add_css_class("flat");
    {
        let row = row.clone();
        rm.connect_clicked(move |_| {
            if let Some(Ok(box_)) = row.parent().map(|p| p.downcast::<gtk::Box>()) {
                box_.remove(&row);
            }
        });
    }
    row.append(&entry);
    row.append(&rm);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_changes_do_not_rebuild_the_catalog() {
        let before = Config::default();
        let mut appearance = before.clone();
        appearance.zoom = appearance.zoom.saturating_add(1);
        appearance.spacing = 7;
        assert!(!catalog_settings_changed(
            &before.exclude,
            &before.include,
            &appearance
        ));

        appearance.exclude.push("target".into());
        assert!(catalog_settings_changed(
            &before.exclude,
            &before.include,
            &appearance
        ));
    }
}
