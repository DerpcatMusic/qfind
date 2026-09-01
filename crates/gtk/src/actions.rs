//! Open / reveal / preview / clipboard — file-manager conventions.
//!
//! Reveal uses GTK FileLauncher (FileManager1.ShowItems under the hood) and
//! falls back to opening the parent folder. Preview tries GNOME Sushi
//! (`org.gnome.NautilusPreviewer2.ShowFile`, then `sushi`), then a small
//! built-in window.

use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::ToVariant;
use gtk::prelude::*;
use qfind_core::{Config, OpenHow};

use crate::row::RowData;

type ThumbnailResult = Result<PathBuf, String>;
type SharedThumbnail = Arc<Mutex<ThumbnailState>>;

#[derive(Default)]
struct ThumbnailState {
    result: Option<ThumbnailResult>,
    wakers: Vec<Waker>,
}

struct ThumbnailWait(SharedThumbnail);

impl Future for ThumbnailWait {
    type Output = ThumbnailResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(result) = state.result.clone() {
            Poll::Ready(result)
        } else {
            if !state.wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
                state.wakers.push(cx.waker().clone());
            }
            Poll::Pending
        }
    }
}

struct ThumbnailJob {
    path: PathBuf,
    output: PathBuf,
    width: u32,
    height: u32,
    result: SharedThumbnail,
}

fn thumbnail_jobs() -> &'static Mutex<HashMap<PathBuf, SharedThumbnail>> {
    static JOBS: OnceLock<Mutex<HashMap<PathBuf, SharedThumbnail>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn thumbnail_sender() -> &'static mpsc::Sender<ThumbnailJob> {
    static SENDER: OnceLock<mpsc::Sender<ThumbnailJob>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<ThumbnailJob>();
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = thread::available_parallelism()
            .map_or(2, usize::from)
            .div_ceil(2)
            .clamp(2, 8);
        for _ in 0..workers {
            let receiver = Arc::clone(&receiver);
            thread::spawn(move || {
                loop {
                    let job = receiver
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .recv();
                    let Ok(job) = job else { return };
                    let rendered = render_thumbnail(&job.path, job.width, job.height, &job.output);
                    let wakers = {
                        let mut state = job
                            .result
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.result = Some(rendered);
                        std::mem::take(&mut state.wakers)
                    };
                    for waker in wakers {
                        waker.wake();
                    }
                    thumbnail_jobs()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&job.output);
                }
            });
        }
        sender
    })
}

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

pub fn trash(path: &str) {
    gio::File::for_path(path).trash_async(
        glib::Priority::DEFAULT,
        None::<&gio::Cancellable>,
        |result| {
            if let Err(error) = result {
                eprintln!("qfind: could not move item to trash: {error}");
            }
        },
    );
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

    let child = preview_widget(Path::new(path));
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

pub(crate) fn preview_widget(p: &Path) -> gtk::Widget {
    if p.is_dir() {
        return fallback_preview(p, "inode/directory").upcast();
    }
    let (ctype, _) = gio::content_type_guess(Some(p), None::<&[u8]>);
    if can_thumbnail(p) {
        thumbnail_preview(p).upcast()
    } else if is_textish(&ctype, p) {
        text_preview(p).upcast()
    } else {
        fallback_preview(p, &ctype).upcast()
    }
}

pub(crate) fn load_thumbnail(
    stack: &gtk::Stack,
    picture: &gtk::Picture,
    path: &Path,
    width: u32,
    height: u32,
) {
    let token = format!("{}#{width}x{height}", path.display());
    stack.set_widget_name(&token);
    stack.set_visible_child_name("icon");
    if !can_thumbnail(path) {
        return;
    }
    let Ok(output) = thumbnail_output(path, width, height) else {
        return;
    };
    if output.is_file() {
        picture.set_filename(Some(output));
        stack.set_visible_child_name("picture");
        return;
    }
    let (result, start) = {
        let mut jobs = thumbnail_jobs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(result) = jobs.get(&output) {
            (Arc::clone(result), false)
        } else {
            let result = Arc::new(Mutex::new(ThumbnailState::default()));
            jobs.insert(output.clone(), Arc::clone(&result));
            (result, true)
        }
    };
    if start {
        let job = ThumbnailJob {
            path: path.to_path_buf(),
            output: output.clone(),
            width,
            height,
            result: Arc::clone(&result),
        };
        if thumbnail_sender().send(job).is_err() {
            thumbnail_jobs()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&output);
            return;
        }
    }
    let stack = stack.clone();
    let picture = picture.clone();
    glib::MainContext::default().spawn_local(async move {
        if let Ok(rendered) = ThumbnailWait(result).await {
            if stack.widget_name() == token {
                picture.set_filename(Some(rendered));
                stack.set_visible_child_name("picture");
            }
        }
    });
}

fn thumbnail_preview(path: &Path) -> gtk::Stack {
    let stack = gtk::Stack::new();
    let fallback = fallback_preview(path, "application/octet-stream");
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    stack.add_named(&fallback, Some("icon"));
    stack.add_named(&picture, Some("picture"));
    load_thumbnail(&stack, &picture, path, 1000, 800);
    stack
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_raster_image(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "bmp" | "gif" | "ico" | "jpeg" | "jpg" | "png" | "tif" | "tiff" | "webp"
    )
}

fn can_thumbnail(path: &Path) -> bool {
    is_raster_image(path)
        || matches!(
            extension(path).as_str(),
            "svg"
                | "svgz"
                | "pdf"
                | "ps"
                | "eps"
                | "djvu"
                | "xps"
                | "mp3"
                | "flac"
                | "wav"
                | "ogg"
                | "m4a"
                | "aac"
                | "aiff"
                | "opus"
                | "wma"
                | "mp4"
                | "mkv"
                | "webm"
                | "mov"
                | "avi"
                | "m4v"
                | "doc"
                | "docx"
                | "odt"
                | "ods"
                | "odp"
                | "ppt"
                | "pptx"
                | "xls"
                | "xlsx"
        )
}

fn thumbnail_output(path: &Path, width: u32, height: u32) -> ThumbnailResult {
    let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    meta.len().hash(&mut hash);
    meta.modified().ok().hash(&mut hash);
    width.hash(&mut hash);
    height.hash(&mut hash);
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("qfind/thumbnails");
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    Ok(cache.join(format!("{:016x}.png", hash.finish())))
}

fn render_thumbnail(path: &Path, width: u32, height: u32, output: &Path) -> ThumbnailResult {
    let ext = extension(path);
    if is_raster_image(path) {
        image::ImageReader::open(path)
            .map_err(|error| error.to_string())?
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .decode()
            .map_err(|error| error.to_string())?
            .thumbnail(width.max(1), height.max(1))
            .save(output)
            .map_err(|error| error.to_string())?;
        return Ok(output.to_path_buf());
    }
    let size = width.max(height).min(1600).to_string();
    let mut command = if matches!(ext.as_str(), "svg" | "svgz") {
        let mut command = Command::new("rsvg-convert");
        command
            .args(["--format", "png", "--keep-aspect-ratio", "--width"])
            .arg(width.to_string())
            .arg("--height")
            .arg(height.to_string())
            .arg("--output")
            .arg(output)
            .arg(path);
        command
    } else if matches!(ext.as_str(), "pdf" | "ps" | "eps" | "djvu" | "xps") {
        let mut command = Command::new("evince-thumbnailer");
        command.arg("-s").arg(&size).arg(path).arg(output);
        command
    } else if matches!(
        ext.as_str(),
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "aiff" | "opus" | "wma"
    ) {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(path)
            .args([
                "-filter_complex",
                &format!(
                    "aformat=channel_layouts=mono,showwavespic=s={width}x{height}:colors=#8aa4ff"
                ),
                "-frames:v",
                "1",
            ])
            .arg(output);
        command
    } else if matches!(ext.as_str(), "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v") {
        let mut command = Command::new("ffmpegthumbnailer");
        command
            .args(["-i"])
            .arg(path)
            .args(["-o"])
            .arg(output)
            .args(["-s", &size, "-c", "png", "-t", "10%"]);
        command
    } else {
        let mut command = Command::new("gsf-office-thumbnailer");
        command
            .arg("-i")
            .arg(path)
            .arg("-o")
            .arg(output)
            .arg("-s")
            .arg(&size);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() && output.is_file() => {
                return Ok(output.to_path_buf());
            }
            Ok(Some(status)) => return Err(format!("thumbnailer exited with {status}")),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("thumbnail timed out".into());
            }
            Err(error) => return Err(error.to_string()),
        }
    }
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
