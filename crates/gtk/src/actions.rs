//! Open / reveal / preview / clipboard — file-manager conventions.
//!
//! Reveal uses GTK FileLauncher (FileManager1.ShowItems under the hood) and
//! falls back to opening the parent folder. Preview tries GNOME Sushi
//! (`org.gnome.NautilusPreviewer2.ShowFile`, then `sushi`), then a small
//! built-in window.

use std::path::Path;
use std::process::Command;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::ToVariant;
use gtk::prelude::*;
use qfind_core::{Config, OpenHow};

use crate::row::RowData;

pub(crate) fn content_for_path(path: &str) -> Option<gdk::ContentProvider> {
    let file = gio::File::for_path(path);
    let uri = format!("{}\r\n", file.uri());
    let bytes = glib::Bytes::from(uri.as_bytes());
    let uris = gdk::ContentProvider::for_bytes("text/uri-list", &bytes);
    let typed = gdk::ContentProvider::for_value(&file.to_value());
    Some(gdk::ContentProvider::new_union(&[typed, uris]))
}

pub fn selected_row(selection: &gtk::SingleSelection) -> Option<RowData> {
    selection.selected_item().and_downcast::<RowData>()
}

pub fn open(window: &impl IsA<gtk::Window>, path: &str) {
    let cfg = Config::load();
    let is_dir = Path::new(path).is_dir();
    if let OpenHow::Editor { program, args } = cfg.open_how(Path::new(path), is_dir) {
        if Command::new(&program).args(&args).arg(path).spawn().is_ok() {
            return;
        }
    }
    let file = gio::File::for_path(path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.launch(Some(window), None::<&gio::Cancellable>, |_| {});
}

pub fn open_with(window: &impl IsA<gtk::Window>, path: &str) {
    let file = gio::File::for_path(path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.set_always_ask(true);
    launcher.launch(Some(window), None::<&gio::Cancellable>, |_| {});
}

/// Highlight the Hit in the default file manager (Nautilus, Dolphin, Thunar, …).
pub fn reveal(window: &impl IsA<gtk::Window>, path: &str) {
    let file = gio::File::for_path(path);
    if show_items_dbus(&file) {
        return;
    }
    let launcher = gtk::FileLauncher::new(Some(&file));
    let win = window.clone().upcast::<gtk::Window>();
    launcher.open_containing_folder(Some(&win), None::<&gio::Cancellable>, move |res| {
        if res.is_err() {
            if let Some(parent) = Path::new(&file.path().unwrap_or_default()).parent() {
                let dir = gio::File::for_path(parent);
                let open = gtk::FileLauncher::new(Some(&dir));
                open.launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
            }
        }
    });
}

pub fn open_folder(window: &impl IsA<gtk::Window>, path: &str, is_dir: bool) {
    let target = if is_dir {
        Path::new(path).to_path_buf()
    } else {
        Path::new(path)
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf()
    };
    let file = gio::File::for_path(&target);
    if show_folders_dbus(&file) {
        return;
    }
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.launch(Some(window), None::<&gio::Cancellable>, |_| {});
}

pub fn copy_text(text: &str) {
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

pub fn copy_path(path: &str) {
    copy_text(path);
}

pub fn copy_name(name: &str) {
    copy_text(name);
}

pub fn copy_uri(path: &str) {
    let uri = gio::File::for_path(path).uri();
    copy_text(&uri);
}

/// Spacebar Quick Look. Sushi first; built-in window if it is missing.
pub fn preview(parent: &gtk::Window, path: &str, slot: &std::cell::RefCell<Option<gtk::Window>>) {
    if let Some(existing) = slot.borrow_mut().take() {
        existing.close();
        return;
    }
    if sushi_show(path) {
        return;
    }
    let win = builtin_preview(parent, path);
    *slot.borrow_mut() = Some(win);
}

fn sushi_show(path: &str) -> bool {
    let uri = gio::File::for_path(path).uri();
    dbus_show_file(&uri)
}

fn dbus_show_file(uri: &str) -> bool {
    let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else {
        return false;
    };
    let args = (uri, "", true).to_variant();
    conn.call_sync(
        Some("org.gnome.NautilusPreviewer"),
        "/org/gnome/NautilusPreviewer",
        "org.gnome.NautilusPreviewer2",
        "ShowFile",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        800,
        None::<&gio::Cancellable>,
    )
    .is_ok()
}

fn show_items_dbus(file: &gio::File) -> bool {
    file_manager1("ShowItems", file)
}

fn show_folders_dbus(file: &gio::File) -> bool {
    file_manager1("ShowFolders", file)
}

fn file_manager1(method: &str, file: &gio::File) -> bool {
    let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else {
        return false;
    };
    let uri = file.uri();
    let args = (vec![uri.as_str()], "").to_variant();
    conn.call_sync(
        Some("org.freedesktop.FileManager1"),
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        1200,
        None::<&gio::Cancellable>,
    )
    .is_ok()
}

fn builtin_preview(parent: &gtk::Window, path: &str) -> gtk::Window {
    let win = gtk::Window::builder()
        .transient_for(parent)
        .title(
            Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Preview"),
        )
        .default_width(780)
        .default_height(560)
        .build();

    let p = Path::new(path);
    let (ctype, _) = gio::content_type_guess(Some(p), None::<&[u8]>);
    let child: gtk::Widget = if ctype.starts_with("image/") {
        let pic = gtk::Picture::for_filename(p);
        pic.set_content_fit(gtk::ContentFit::Contain);
        pic.upcast()
    } else if is_textish(&ctype, p) {
        text_preview(p).upcast()
    } else {
        fallback_preview(p, &ctype).upcast()
    };
    win.set_child(Some(&child));

    let esc = gtk::EventControllerKey::new();
    let w = win.clone();
    esc.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape || key == gdk::Key::space {
            w.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    win.add_controller(esc);
    win.present();
    win
}

fn is_textish(ctype: &str, path: &Path) -> bool {
    ctype.starts_with("text/")
        || ctype.contains("json")
        || ctype.contains("xml")
        || matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("md" | "rs" | "toml" | "json" | "txt" | "css" | "js" | "ts" | "py" | "sh")
        )
}

fn text_preview(path: &Path) -> gtk::ScrolledWindow {
    let text = std::fs::read(path)
        .ok()
        .map(|b| {
            let cut = b.len().min(64 * 1024);
            String::from_utf8_lossy(&b[..cut]).into_owned()
        })
        .unwrap_or_else(|| "(unreadable)".into());
    let view = gtk::TextView::builder()
        .editable(false)
        .wrap_mode(gtk::WrapMode::Word)
        .monospace(true)
        .left_margin(10)
        .right_margin(10)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    view.buffer().set_text(&text);
    gtk::ScrolledWindow::builder().child(&view).build()
}

fn fallback_preview(path: &Path, ctype: &str) -> gtk::Box {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 12);
    v.set_valign(gtk::Align::Center);
    v.set_halign(gtk::Align::Center);
    v.set_margin_top(24);
    v.set_margin_bottom(24);
    let icon = gtk::Image::from_gicon(&gio::content_type_get_icon(ctype));
    icon.set_pixel_size(64);
    let name = gtk::Label::new(
        path.file_name()
            .and_then(|n| n.to_str())
            .or_else(|| path.to_str()),
    );
    name.add_css_class("title-2");
    let meta = gtk::Label::new(Some(&format!(
        "{ctype}  ·  {}",
        path.parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    )));
    meta.add_css_class("dim-label");
    v.append(&icon);
    v.append(&name);
    v.append(&meta);
    v
}

#[allow(dead_code)]
pub fn human_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let n = n as f64;
    if n < K {
        format!("{} B", n as u64)
    } else if n < K * K {
        format!("{:.1} KB", n / K)
    } else if n < K * K * K {
        format!("{:.1} MB", n / (K * K))
    } else {
        format!("{:.1} GB", n / (K * K * K))
    }
}

#[allow(dead_code)]
pub fn human_mtime(secs: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(secs);
    let delta = now.saturating_sub(secs);
    if delta < 45 {
        "just now".into()
    } else if delta < 90 {
        "1 min ago".into()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 3600 * 36 {
        format!("{} h ago", (delta + 1800) / 3600)
    } else if delta < 86400 * 14 {
        format!("{} days ago", (delta + 43200) / 86400)
    } else {
        format!("{} days ago", delta / 86400)
    }
}

#[allow(dead_code)]
pub fn format_meta(path: &str) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let size = human_size(m.len());
            let when = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| human_mtime(d.as_secs() as i64))
                .unwrap_or_default();
            if when.is_empty() {
                size
            } else {
                format!("{size}  ·  {when}")
            }
        }
        Err(_) => String::new(),
    }
}
