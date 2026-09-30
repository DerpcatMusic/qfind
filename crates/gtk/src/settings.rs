//! Settings window: Catalog, PreviewMode, and opening behavior.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use qfind_core::appearance::{
    APPEARANCES, THEMES, accent_for, normalize_appearance, normalize_gtk_theme, normalize_theme,
};
use qfind_core::{Config, MatchMode, OpenMode, PreviewMode};

thread_local! {
    /// The desktop's own theme choices, read once before we change anything.
    static DESKTOP: Desktop = Desktop::read();
    /// The one provider for our stylesheet. A new provider per apply stacked
    /// every past Appearance on top of the next, so the custom palette never
    /// went away once it had been loaded.
    static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
    static PROVIDER_ADDED: Cell<bool> = const { Cell::new(false) };
    /// One Settings window at a time. Each used to carry its own widget state
    /// and its own `on_save`, so saving in one could Rebuild the Catalog out
    /// from under another that was still open and stale.
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

/// The desktop's GTK theme, icon theme and color scheme.
///
/// GTK only reads `settings.ini` when the portal does not forward
/// `org.gnome.desktop.interface` (Hyprland's does not), so a desktop set up
/// through gsettings — adw-gtk3 recolored by the shell, Papirus icons — looked
/// like stock GTK here. gsettings wins when it names a theme GTK 4 can load.
struct Desktop {
    gtk_theme: Option<String>,
    icon_theme: Option<String>,
    prefer_dark: bool,
}

impl Desktop {
    fn read() -> Self {
        let settings = gtk::Settings::default();
        let gsettings = gtk::gio::SettingsSchemaSource::default()
            .and_then(|source| source.lookup("org.gnome.desktop.interface", true))
            .map(|_| gtk::gio::Settings::new("org.gnome.desktop.interface"));
        let key = |name: &str| {
            gsettings
                .as_ref()
                .map(|settings| settings.string(name).to_string())
                .filter(|value| !value.is_empty())
        };
        let gtk_theme = key("gtk-theme")
            .filter(|name| gtk_themes().contains(name))
            .or_else(|| {
                settings
                    .as_ref()
                    .and_then(|s| s.gtk_theme_name())
                    .map(Into::into)
            });
        let icon_theme = key("icon-theme")
            .filter(|name| icon_themes().contains(name))
            .or_else(|| {
                settings
                    .as_ref()
                    .and_then(|s| s.gtk_icon_theme_name())
                    .map(Into::into)
                    .filter(|name: &String| name != "QfindWorkspace")
            });
        let prefer_dark = key("color-scheme").map_or_else(
            || {
                settings
                    .as_ref()
                    .is_some_and(|s| s.is_gtk_application_prefer_dark_theme())
            },
            |scheme| scheme == "prefer-dark",
        );
        Self {
            gtk_theme,
            icon_theme,
            prefer_dark,
        }
    }
}

/// `$XDG_DATA_HOME`, `~/.<legacy>`, then `$XDG_DATA_DIRS`, each joined with `sub`.
fn data_dirs(sub: &str, legacy: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join(sub));
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(legacy));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').map(|dir| PathBuf::from(dir).join(sub)));
    dirs
}

/// Installed directories under `sub` that pass `keep`, sorted and unique.
fn installed(sub: &str, legacy: &str, keep: impl Fn(&std::path::Path) -> bool) -> Vec<String> {
    let mut names: Vec<String> = data_dirs(sub, legacy)
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|entry| keep(&entry.path()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// GTK themes that ship a GTK 4 stylesheet, plus GTK's built-in one.
pub fn gtk_themes() -> Vec<String> {
    let mut themes = installed("themes", ".themes", |path| path.join("gtk-4.0").is_dir());
    if !themes.iter().any(|name| name == "Adwaita") {
        themes.insert(0, "Adwaita".into());
    }
    themes
}

/// Icon themes with icons in them (not cursor-only, not hidden, not hicolor).
pub fn icon_themes() -> Vec<String> {
    installed("icons", ".icons", |path| {
        path.file_name()
            .is_some_and(|name| name != "hicolor" && name != "default")
            && std::fs::read_to_string(path.join("index.theme")).is_ok_and(|index| {
                index.lines().any(|line| line.starts_with("Directories="))
                    && !index.lines().any(|line| line.trim() == "Hidden=true")
            })
    })
}

/// `native`: leave the toolkit theme alone, take its accent.
pub fn is_native(cfg: &Config) -> bool {
    normalize_appearance(&cfg.appearance) == "native"
}

/// Whether folders use Megaman's own art rather than the icon theme's.
pub fn bundled_icons(cfg: &Config) -> bool {
    match cfg.icon_theme.trim() {
        "" => !is_native(cfg),
        name => name.eq_ignore_ascii_case("megaman"),
    }
}

/// Whether the current GTK theme (or the user's gtk.css) defines `name`.
fn theme_defines(name: &str) -> bool {
    #[allow(deprecated)]
    gtk::Label::new(None)
        .style_context()
        .lookup_color(name)
        .is_some()
}

/// Surface colors for our stylesheet. libadwaita-style themes (adw-gtk3, and
/// shell palettes such as Noctalia or matugen written into gtk.css) name their
/// sidebar and view colors; GTK's built-in theme does not, so derive those.
fn palette() -> String {
    let named = |name: &str, fallback: &str| {
        if theme_defines(name) {
            format!("@{name}")
        } else {
            fallback.to_owned()
        }
    };
    [
        ("qfind_window", named("window_bg_color", "@theme_bg_color")),
        ("qfind_view", named("view_bg_color", "@theme_base_color")),
        (
            "qfind_sidebar",
            named(
                "sidebar_bg_color",
                "mix(@theme_bg_color, @theme_base_color, 0.5)",
            ),
        ),
        ("qfind_fg", named("window_fg_color", "@theme_fg_color")),
    ]
    .into_iter()
    .map(|(name, value)| format!("@define-color {name} {value};\n"))
    .collect()
}

/// The CSS color for the accent: a custom `#rrggbb`, a Theme preset, or the
/// toolkit's own accent.
fn accent(cfg: &Config) -> String {
    let wanted = cfg.accent.trim();
    if !wanted.is_empty()
        && !wanted.eq_ignore_ascii_case("system")
        && gtk::gdk::RGBA::parse(wanted).is_ok()
    {
        return wanted.to_owned();
    }
    if wanted.eq_ignore_ascii_case("system") || (wanted.is_empty() && is_native(cfg)) {
        // Every libadwaita-style theme and shell palette (adw-gtk3, Noctalia,
        // matugen) defines `accent_bg_color`; GTK's built-in theme does not.
        return if theme_defines("accent_bg_color") {
            "@accent_bg_color"
        } else {
            "@theme_selected_bg_color"
        }
        .to_owned();
    }
    accent_for(&cfg.theme).to_owned()
}

/// Install the stylesheet, icons, and GTK theme for `cfg`. Cheap; call on save.
///
/// Every choice is applied from the desktop's baseline each time, so switching
/// back to `system` really returns to the desktop instead of keeping whatever
/// was set last.
pub fn apply_appearance(cfg: &Config) {
    let (Some(display), Some(settings)) = (gtk::gdk::Display::default(), gtk::Settings::default())
    else {
        return;
    };
    DESKTOP.with(|desktop| {
        let theme = match normalize_gtk_theme(&cfg.gtk_theme) {
            "system" => desktop.gtk_theme.clone(),
            name => Some(name.to_owned()),
        };
        settings.set_gtk_theme_name(theme.as_deref());
        settings.set_gtk_application_prefer_dark_theme(match cfg.color_scheme.trim() {
            "light" => false,
            "dark" => true,
            _ => desktop.prefer_dark,
        });
        let icons = match cfg.icon_theme.trim() {
            "" | "system" => desktop.icon_theme.clone(),
            name if name.eq_ignore_ascii_case("megaman") => desktop.icon_theme.clone(),
            name => Some(name.to_owned()),
        };
        settings.set_gtk_icon_theme_name(icons.as_deref());
    });
    crate::icons::install(bundled_icons(cfg));

    let css = format!(
        "{}@define-color qfind_accent {};\n{}",
        palette(),
        accent(cfg),
        sheet_for(cfg)
    );
    PROVIDER.with(|provider| {
        provider.load_from_string(&css);
        if !PROVIDER_ADDED.replace(true) {
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
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

/// A drop-down over `(value, label)` choices, showing `current` (or the first).
fn choice_drop(choices: &[(String, String)], current: &str) -> gtk::DropDown {
    let labels: Vec<&str> = choices.iter().map(|(_, label)| label.as_str()).collect();
    let drop = gtk::DropDown::from_strings(&labels);
    drop.set_enable_search(choices.len() > 8);
    let position = choices
        .iter()
        .position(|(value, _)| value.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    drop.set_selected(position as u32);
    drop
}

fn chosen(choices: &[(String, String)], drop: &gtk::DropDown) -> String {
    choices
        .get(drop.selected() as usize)
        .map(|(value, _)| value.clone())
        .unwrap_or_default()
}

fn hex(color: &gtk::gdk::RGBA) -> String {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(color.red()),
        byte(color.green()),
        byte(color.blue())
    )
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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
    // Style: index 0 is `custom`, 1 is `native`, as in APPEARANCES.
    let appearance_drop = gtk::DropDown::from_strings(&["Megaman", "Desktop"]);
    appearance_drop.set_selected(index_of(APPEARANCES, normalize_appearance(&cfg.appearance)));

    let (desktop_theme, desktop_icons) = DESKTOP.with(|desktop| {
        (
            desktop.gtk_theme.clone().unwrap_or_default(),
            desktop.icon_theme.clone().unwrap_or_default(),
        )
    });
    // Choices are `(config value, label)`; index 0 is always "follow".
    let gtk_theme_choices: Vec<(String, String)> =
        std::iter::once(("system".to_owned(), format!("System ({desktop_theme})")))
            .chain(gtk_themes().into_iter().map(|name| (name.clone(), name)))
            .collect();
    let gtk_theme_drop = choice_drop(&gtk_theme_choices, normalize_gtk_theme(&cfg.gtk_theme));

    let scheme_choices: Vec<(String, String)> =
        [("system", "System"), ("light", "Light"), ("dark", "Dark")]
            .into_iter()
            .map(|(value, label)| (value.to_owned(), label.to_owned()))
            .collect();
    let scheme_drop = choice_drop(&scheme_choices, cfg.color_scheme.trim());

    let icon_choices: Vec<(String, String)> = [
        (String::new(), "Automatic".to_owned()),
        ("megaman".to_owned(), "Megaman".to_owned()),
        ("system".to_owned(), format!("System ({desktop_icons})")),
    ]
    .into_iter()
    .chain(icon_themes().into_iter().map(|name| (name.clone(), name)))
    .collect();
    let icon_drop = choice_drop(&icon_choices, cfg.icon_theme.trim());

    // Accent: automatic, the toolkit's, a Theme preset, or a custom color.
    let accent_choices: Vec<(String, String)> = [
        (String::new(), "Automatic".to_owned()),
        ("system".to_owned(), "System".to_owned()),
    ]
    .into_iter()
    .chain(
        THEMES
            .iter()
            .map(|theme| (format!("theme:{theme}"), capitalize(theme))),
    )
    .chain(std::iter::once(("custom".to_owned(), "Custom".to_owned())))
    .collect();
    let custom_accent = gtk::gdk::RGBA::parse(cfg.accent.trim()).ok();
    let accent_value = if custom_accent.is_some() {
        "custom".to_owned()
    } else if cfg.accent.trim().eq_ignore_ascii_case("preset")
        || (cfg.accent.trim().is_empty() && !is_native(&cfg))
    {
        format!("theme:{}", normalize_theme(&cfg.theme))
    } else {
        cfg.accent.trim().to_lowercase()
    };
    let accent_drop = choice_drop(&accent_choices, &accent_value);
    let accent_color =
        gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    accent_color.set_rgba(&custom_accent.unwrap_or_else(|| {
        gtk::gdk::RGBA::parse(accent_for(&cfg.theme)).unwrap_or(gtk::gdk::RGBA::BLUE)
    }));
    accent_color.set_visible(accent_value == "custom");
    {
        let accent_color = accent_color.clone();
        let accent_choices = accent_choices.clone();
        accent_drop.connect_selected_notify(move |drop| {
            accent_color.set_visible(
                accent_choices
                    .get(drop.selected() as usize)
                    .is_some_and(|(value, _)| value == "custom"),
            );
        });
    }
    let accent_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    accent_box.append(&accent_drop);
    accent_box.append(&accent_color);

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
        "Style",
        "Megaman adds its palette and ~/.config/qfind/custom.css; Desktop is the GTK theme alone.",
        &appearance_drop,
    );
    row(
        &appearance.1,
        "GTK theme",
        "System follows the desktop, including shell palettes such as Noctalia or matugen.",
        &gtk_theme_drop,
    );
    row(
        &appearance.1,
        "Color scheme",
        "Light or dark variant of the GTK theme.",
        &scheme_drop,
    );
    row(
        &appearance.1,
        "Icons",
        "Automatic uses Megaman's icons with its Style, the desktop's otherwise.",
        &icon_drop,
    );
    row(
        &appearance.1,
        "Accent",
        "Presets are shared with the TUI. System takes the GTK theme's accent.",
        &accent_box,
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
        let gtk_theme_drop = gtk_theme_drop.clone();
        let scheme_drop = scheme_drop.clone();
        let icon_drop = icon_drop.clone();
        let accent_drop = accent_drop.clone();
        let accent_color = accent_color.clone();
        let gtk_theme_choices = gtk_theme_choices.clone();
        let scheme_choices = scheme_choices.clone();
        let icon_choices = icon_choices.clone();
        let accent_choices = accent_choices.clone();
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
            cfg.gtk_theme = chosen(&gtk_theme_choices, &gtk_theme_drop);
            cfg.color_scheme = chosen(&scheme_choices, &scheme_drop);
            cfg.icon_theme = chosen(&icon_choices, &icon_drop);
            let accent = chosen(&accent_choices, &accent_drop);
            cfg.accent = if let Some(theme) = accent.strip_prefix("theme:") {
                cfg.theme = theme.to_owned();
                // An explicit preset, so it also applies with the Desktop style.
                "preset".to_owned()
            } else if accent == "custom" {
                hex(&accent_color.rgba())
            } else {
                accent
            };
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
        let gtk_theme_drop = gtk_theme_drop.clone();
        let scheme_drop = scheme_drop.clone();
        let icon_drop = icon_drop.clone();
        let accent_drop = accent_drop.clone();
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
            gtk_theme_drop.set_selected(0);
            scheme_drop.set_selected(0);
            icon_drop.set_selected(0);
            accent_drop.set_selected(0);
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
    fn icon_choice_follows_style_until_set() {
        let mut cfg = Config::default();
        assert!(bundled_icons(&cfg), "Megaman style defaults to its icons");
        cfg.appearance = "native".into();
        assert!(
            !bundled_icons(&cfg),
            "Desktop style defaults to the desktop's"
        );
        cfg.icon_theme = "Megaman".into();
        assert!(bundled_icons(&cfg));
        cfg.icon_theme = "Papirus-Dark".into();
        assert!(!bundled_icons(&cfg));
        assert_eq!(hex(&gtk::gdk::RGBA::new(1.0, 0.5, 0.0, 1.0)), "#ff8000");
    }

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
