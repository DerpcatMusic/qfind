//! Open / reveal / preview / clipboard — file-manager conventions.
//!
//! Reveal opens Megaman's own browser at the containing folder. Preview tries GNOME Sushi
//! (`org.gnome.NautilusPreviewer2.ShowFile`, then `sushi`), then a small
//! built-in window.

use std::cell::RefCell;
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
                    // A panic in `image` on a corrupt file used to kill the
                    // thread with `state.result` still `None` and the map entry
                    // still present, so every later `load_thumbnail` took the
                    // "already started" branch and awaited a future that could
                    // never resolve: that thumbnail was dead for the session and
                    // the pool shrank by one per bad file.
                    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        render_thumbnail(&job.path, job.width, job.height, &job.output)
                    }))
                    .unwrap_or_else(|_| Err("thumbnail worker panicked".into()));
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
    content_for_paths(&[path.to_owned()])
}

pub(crate) fn content_for_paths(paths: &[String]) -> Option<gdk::ContentProvider> {
    if paths.is_empty() {
        return None;
    }
    let files: Vec<_> = paths.iter().map(gio::File::for_path).collect();
    let uris = files
        .iter()
        .map(|file| format!("{}\r\n", file.uri()))
        .collect::<String>();
    let typed = gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value());
    let uris =
        gdk::ContentProvider::for_bytes("text/uri-list", &glib::Bytes::from(uris.as_bytes()));
    if files.len() == 1 {
        let single = gdk::ContentProvider::for_value(&files[0].to_value());
        Some(gdk::ContentProvider::new_union(&[typed, single, uris]))
    } else {
        Some(gdk::ContentProvider::new_union(&[typed, uris]))
    }
}

pub fn selected_rows(selection: &impl IsA<gtk::SelectionModel>) -> Vec<RowData> {
    let selected = selection.as_ref().selection();
    let model = selection.as_ref().upcast_ref::<gio::ListModel>();
    (0..selected.size())
        .filter_map(|index| {
            model
                .item(selected.nth(index as u32))?
                .downcast::<RowData>()
                .ok()
        })
        .collect()
}

pub fn selected_row(selection: &impl IsA<gtk::SelectionModel>) -> Option<RowData> {
    selected_rows(selection).into_iter().next()
}

pub fn open(window: &impl IsA<gtk::Window>, path: &str) {
    let cfg = Config::load();
    let is_dir = Path::new(path).is_dir();
    if let OpenHow::Editor { program, args } = cfg.open_how(Path::new(path), is_dir) {
        let mut editor = Command::new(&program);
        editor.args(&args).arg(path);
        if spawn_detached(&mut editor).is_ok() {
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
    if open_megaman(&file, false) {
        return;
    }
    let launcher = gtk::FileLauncher::new(Some(&file));
    let win = window.clone().upcast::<gtk::Window>();
    launcher.open_containing_folder(Some(&win), None::<&gio::Cancellable>, move |res| {
        if res.is_err()
            && let Some(parent) = Path::new(&file.path().unwrap_or_default()).parent()
        {
            let dir = gio::File::for_path(parent);
            let open = gtk::FileLauncher::new(Some(&dir));
            open.launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
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
    if open_megaman(&file, true) {
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

pub fn copy_name(name: &str) {
    copy_text(name);
}

pub fn copy_paths(paths: &[String]) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = if paths.len() == 1 {
        content_for_path(&paths[0])
    } else {
        content_for_paths(paths)
    };
    if let Some(provider) = provider {
        let _ = display.clipboard().set_content(Some(&provider));
    }
}

/// Spacebar Quick Look. Sushi first; built-in window if it declines or is
/// missing.
pub fn preview(parent: &gtk::Window, path: &str, slot: &std::cell::RefCell<Option<gtk::Window>>) {
    // A window the user closed must not sit in the slot: `take()` would hand it
    // back, the next Space would close it again, and only the Space after that
    // would open anything.
    if let Some(existing) = slot.borrow_mut().take()
        && existing.is_visible()
    {
        existing.close();
        return;
    }
    let parent = parent.clone();
    let path = path.to_owned();
    let slot = slot.clone();
    glib::MainContext::default().spawn_local(async move {
        // Audio and video go straight to the built-in surfaces. Handing them to
        // Sushi first meant the file left as a generic icon window, with no
        // waveform and no transport.
        if is_media(Path::new(&path)) {
            let win = builtin_preview(&parent, &path);
            *slot.borrow_mut() = Some(win);
            return;
        }
        // The D-Bus round trip is sync but happens on a worker: `bus_get_sync`
        // alone can stall for GIO's 25 s bus-acquire timeout, which used to
        // freeze the whole window on every Space with no reachable session bus.
        let uri = gio::File::for_path(&path).uri();
        let accepted = gio::spawn_blocking(move || sushi_show(&uri))
            .await
            .unwrap_or(false);
        if accepted {
            return;
        }
        let win = builtin_preview(&parent, &path);
        *slot.borrow_mut() = Some(win);
    });
}

/// Ask Sushi to show the file, reporting whether it *accepted* it.
fn sushi_show(uri: &str) -> bool {
    let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    let args = (uri, "", true).to_variant();
    let Ok(reply) = conn.call_sync(
        Some("org.gnome.NautilusPreviewer"),
        "/org/gnome/NautilusPreviewer",
        "org.gnome.NautilusPreviewer2",
        "ShowFile",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        800,
        gio::Cancellable::NONE,
    ) else {
        return false;
    };
    // The reply is a boolean. It used to be ignored, so a Sushi that declined
    // the type made Space do nothing at all with no fallback.
    reply.child_value(0).get::<bool>().unwrap_or(false)
}

/// Reuse this Megaman instance to show `file`'s folder.
///
/// Spawning a *new* process was two bugs at once: the binary name was
/// hardcoded, so a symlinked, renamed, or out-of-`$PATH` install silently fell
/// through to Nautilus, and when it did work every Reveal produced a second
/// full app instance with its own Catalog and timers. Asking this process to
/// navigate does the right thing either way.
fn open_megaman(file: &gio::File, directory: bool) -> bool {
    let Some(path) = file.path() else {
        return false;
    };
    let target = if directory {
        path
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf()
    };
    NAVIGATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_mut() {
            Some(navigate) => {
                navigate(target);
                true
            }
            None => false,
        }
    })
}

thread_local! {
    /// The active window's navigator, set once the shell is built.
    static NAVIGATE: RefCell<Option<Box<dyn Fn(PathBuf)>>> = const { RefCell::new(None) };
}

/// Run a helper without leaving it behind.
///
/// Dropping a `Child` never waits for it, so every editor launch, every Reveal,
/// and every terminal left a `defunct` entry in the process table for the life
/// of the app. One thread per helper is nothing next to the process it waits on,
/// and it is the only thing that can reap *our* children.
pub fn spawn_detached(command: &mut Command) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Publish the active window's navigator so "Show in Files" and "Open Folder"
/// can navigate here instead of spawning another process.
pub fn set_navigator(navigate: std::rc::Rc<std::cell::RefCell<Box<dyn Fn(PathBuf)>>>) {
    NAVIGATE.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |path| navigate.borrow()(path)));
    });
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

    // `true`: Space is a request to hear or watch this, so the window that
    // Space opened starts playing.
    let child = preview_widget(Path::new(path), true);
    win.set_child(Some(&child));

    // Escape closes. Space deliberately does not: it belongs to the media
    // surfaces, where it plays and pauses, and it used to close the window
    // instead whenever the Preview itself did not want it.
    let esc = gtk::EventControllerKey::new();
    let w = win.clone();
    esc.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    win.add_controller(esc);
    win.present();
    // Without this the keyboard stayed in the window behind, and a second Space
    // was typed into the search box instead of pausing what Space had opened.
    // Deferred, because moving the focus while the Space that opened this
    // window is still being delivered lands that same Space on the new widget,
    // which cancels the playback the Preview had just started.
    let focused = win.clone();
    glib::idle_add_local_once(move || {
        gtk::prelude::GtkWindowExt::set_focus(&focused, Some(&child));
    });
    win
}

/// Does this app have a real Preview surface for it, rather than a thumbnail?
pub fn is_media(p: &Path) -> bool {
    let (ctype, _) = gio::content_type_guess(Some(p), None::<&[u8]>);
    crate::audio::is_audio(p, &ctype) || crate::preview::is_video(p, &ctype)
}

/// The Preview surface for a file.
///
/// `play` starts media immediately. It is on for the window Space opens and off
/// for the Inspector, which is a look at the file: two players for one file meant
/// the track played twice over itself.
pub(crate) fn preview_widget(p: &Path, play: bool) -> gtk::Widget {
    if p.is_dir() {
        return fallback_preview(p, "inode/directory").upcast();
    }
    // Every surface below this line opens the file: symphonia, playbin, the
    // text reader, the thumbnailer. A fifo, a socket, or a symlink to
    // /dev/zero would block or read forever in at least one of them, so a
    // Preview is only ever built for something a reader can finish.
    if !std::fs::metadata(p).is_ok_and(|meta| meta.is_file()) {
        return fallback_preview(p, "application/octet-stream").upcast();
    }
    let (ctype, _) = gio::content_type_guess(Some(p), None::<&[u8]>);
    // Audio and video get a real surface: decoded waveform with a transport and
    // scrubbing, and in-place playback. Both are checked before the thumbnailer,
    // which would otherwise swallow an `.mp3` and show a generic icon.
    if crate::audio::is_audio(p, &ctype) {
        return crate::preview::audio_preview(p, play);
    }
    if crate::preview::is_video(p, &ctype) {
        return crate::preview::video_preview(p, play);
    }
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
            // Shed load during fast scrolls: decoding catches up when the
            // view settles instead of saturating workers with stale rows.
            if jobs.len() > 64 {
                return;
            }
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
        if let Ok(rendered) = ThumbnailWait(result).await
            && stack.widget_name() == token
        {
            picture.set_filename(Some(rendered));
            stack.set_visible_child_name("picture");
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

/// Where thumbnails live. Created once: `create_dir_all` ran on the main thread
/// for every tile bound, adding a syscall storm to the scroll path.
fn thumbnail_cache() -> ThumbnailResult {
    static CACHE: std::sync::OnceLock<ThumbnailResult> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            // `XDG_CACHE_HOME`, then `$HOME/.cache`, then a per-user directory
            // under the temp dir. The bare `temp_dir()` used as a last resort
            // collided with another user's `qfind` and silently disabled
            // thumbnails for the whole session with no diagnostic.
            let base = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
                .unwrap_or_else(|| {
                    let user = std::env::var("USER")
                        .or_else(|_| std::env::var("LOGNAME"))
                        .unwrap_or_else(|_| "shared".into());
                    std::env::temp_dir().join(format!("qfind-{user}"))
                });
            let cache = base.join("qfind/thumbnails");
            std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
            Ok(cache)
        })
        .clone()
}

fn thumbnail_output(path: &Path, width: u32, height: u32) -> ThumbnailResult {
    let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    meta.len().hash(&mut hash);
    meta.modified().ok().hash(&mut hash);
    width.hash(&mut hash);
    height.hash(&mut hash);
    Ok(thumbnail_cache()?.join(format!("{:016x}.png", hash.finish())))
}

/// Render a thumbnail, atomically.
///
/// Everything is written to `<cache>/x.png.part` and renamed into place only on
/// success. A thumbnailer killed at the 3 s timeout used to leave a half-written
/// PNG at the final path; the cache key still matched on the next bind, so that
/// corrupt image was shown permanently with no way to recover short of clearing
/// the whole thumbnail cache by hand.
fn render_thumbnail(path: &Path, width: u32, height: u32, cache: &Path) -> ThumbnailResult {
    let output = cache.with_extension("png.part");
    let ext = extension(path);
    match render_into(path, &ext, width, height, &output) {
        Ok(()) => match std::fs::rename(&output, cache.with_extension("png")) {
            Ok(()) => Ok(cache.with_extension("png")),
            Err(error) => {
                let _ = std::fs::remove_file(&output);
                Err(error.to_string())
            }
        },
        Err(error) => {
            let _ = std::fs::remove_file(&output);
            Err(error)
        }
    }
}

/// Refuse to decode anything larger than this. A 100 MP photo is 400 MB once
/// decoded, and eight of those on eight worker threads will exhaust RAM.
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;

fn render_into(
    path: &Path,
    ext: &str,
    width: u32,
    height: u32,
    output: &Path,
) -> Result<(), String> {
    if is_raster_image(path) {
        if let Ok(meta) = std::fs::metadata(path)
            && meta.len() > MAX_DECODE_BYTES
        {
            return Err(format!("image too large to thumbnail: {}", path.display()));
        }
        // `image` decodes the full image before `thumbnail` shrinks it, so the
        // guard above is what keeps that bounded.
        image::ImageReader::open(path)
            .map_err(|error| error.to_string())?
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .decode()
            .map_err(|error| error.to_string())?
            .thumbnail(width.max(1), height.max(1))
            .save(output)
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let size = width.max(height).min(1600).to_string();
    let mut command = if matches!(ext, "svg" | "svgz") {
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
    } else if matches!(ext, "pdf" | "ps" | "eps" | "djvu" | "xps") {
        let mut command = Command::new("evince-thumbnailer");
        command.arg("-s").arg(&size).arg(path).arg(output);
        command
    } else if matches!(
        ext,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "aiff" | "opus" | "wma"
    ) {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(path)
            .arg("-o")
            .arg(output)
            .arg("-s")
            .arg(&size);
        command
    } else {
        return Err(format!("no thumbnailer for {ext}"));
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            // Reap the child on every exit path so it cannot become a zombie.
            Ok(Some(status)) if status.success() && output.is_file() => return Ok(()),
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

/// How much of a text file a Preview shows.
const TEXT_PREVIEW_BYTES: usize = 64 * 1024;

/// Read the head of a text file without loading all of it.
///
/// `fs::read` pulled the *entire* file into memory and only then truncated to
/// 64 KiB. `is_textish` matches `.log`, `.json`, `.csv`, and anything GIO guesses
/// as `text/*`, so Space on a multi-gigabyte log allocated gigabytes and froze
/// the window.
fn read_head(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; TEXT_PREVIEW_BYTES + 4];
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    // Do not split a multi-byte scalar: a cut mid-sequence leaves a replacement
    // character at the boundary.
    let mut end = filled.min(TEXT_PREVIEW_BYTES);
    while end > 0 && std::str::from_utf8(&buf[..end]).is_err() {
        end -= 1;
    }
    let truncated = filled > TEXT_PREVIEW_BYTES;
    let mut text = String::from_utf8_lossy(&buf[..end]).into_owned();
    if truncated {
        text.push_str("\n\n… preview truncated");
    }
    Ok(text)
}

fn text_preview(path: &Path) -> gtk::ScrolledWindow {
    let text = read_head(path).unwrap_or_else(|_| "(unreadable)".into());
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
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= K && unit < UNITS.len() - 1 {
        value /= K;
        unit += 1;
    }
    // A 2 TB volume used to read "2048.0 GB": the ladder stopped at gigabytes.
    if unit == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
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
