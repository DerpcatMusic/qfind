use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};

use super::theme::icon_for;
use super::{WorkEvent, reactor};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Kind {
    #[default]
    Text,
    Markdown,
    Image,
}

#[derive(Default)]
pub(crate) struct Side {
    pub(crate) path: Option<PathBuf>,
    pub(crate) body: Vec<String>,
    pub(crate) kind: Kind,
    pub(crate) drag_icon: Option<DragIcon>,
}

pub(crate) struct DragIcon {
    pub(crate) data: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct Thumbnail {
    pub(crate) size: Size,
    pub(crate) preview: Tile,
}

pub(crate) enum Tile {
    Image(Protocol),
    Text(Vec<String>),
    Icon,
}

pub(crate) enum Loaded {
    Image(image::DynamicImage, Option<DragIcon>),
    Text(Kind, Vec<String>),
}

pub(crate) enum Event {
    Side(u64, PathBuf, Result<Loaded, String>),
    Resize(Result<ResizeResponse, ratatui_image::errors::Errors>),
    Thumbnail(u64, PathBuf, Size, Tile),
}

struct SideJob {
    generation: u64,
    path: PathBuf,
}

type ThumbnailJob = (u64, PathBuf, Size);

pub(crate) struct Pipeline {
    pub(crate) side: Side,
    pub(crate) image: ThreadProtocol,
    picker: Picker,
    side_tx: Sender<SideJob>,
    side_pending: Receiver<SideJob>,
    side_generation: Arc<AtomicU64>,
    thumbnail_tx: Sender<ThumbnailJob>,
    thumbnail_epoch: Arc<AtomicU64>,
    thumbnails: HashMap<PathBuf, Thumbnail>,
    thumbnail_order: VecDeque<PathBuf>,
    thumbnail_pending: HashSet<PathBuf>,
    viewport: Vec<(PathBuf, Size)>,
    cache_capacity: usize,
}

impl Pipeline {
    pub(crate) fn new(picker: Picker, events: reactor::Sender<WorkEvent>) -> Self {
        let (resize_tx, resize_rx) = mpsc::channel::<ResizeRequest>();
        let resize_events = events.clone();
        thread::spawn(move || {
            while let Ok(request) = resize_rx.recv() {
                if !resize_events.send(WorkEvent::Preview(Box::new(Event::Resize(
                    request.resize_encode(),
                )))) {
                    break;
                }
            }
        });

        let (side_tx, side_pending) = crossbeam_channel::bounded::<SideJob>(1);
        let side_worker = side_pending.clone();
        let side_generation = Arc::new(AtomicU64::new(0));
        let current_side = Arc::clone(&side_generation);
        let side_events = events.clone();
        thread::spawn(move || {
            while let Ok(mut job) = side_worker.recv() {
                while let Ok(newer) = side_worker.try_recv() {
                    job = newer;
                }
                let generation = job.generation;
                let stale = || current_side.load(Ordering::Relaxed) != generation;
                let loaded = load_side(&job.path, stale);
                if !stale()
                    && !side_events.send(WorkEvent::Preview(Box::new(Event::Side(
                        generation, job.path, loaded,
                    ))))
                {
                    break;
                }
            }
        });

        let workers = thread::available_parallelism().map_or(1, usize::from);
        let (thumbnail_tx, thumbnail_jobs) =
            crossbeam_channel::bounded::<ThumbnailJob>(workers * 8);
        let thumbnail_epoch = Arc::new(AtomicU64::new(0));
        for _ in 0..workers {
            let jobs = thumbnail_jobs.clone();
            let current_epoch = Arc::clone(&thumbnail_epoch);
            let worker_picker = picker.clone();
            let worker_events = events.clone();
            thread::spawn(move || {
                while let Ok((epoch, path, size)) = jobs.recv() {
                    let stale = || current_epoch.load(Ordering::Relaxed) != epoch;
                    if stale() {
                        continue;
                    }
                    let tile = load_tile(&path, size, &worker_picker, stale);
                    if !stale()
                        && !worker_events.send(WorkEvent::Preview(Box::new(Event::Thumbnail(
                            epoch, path, size, tile,
                        ))))
                    {
                        break;
                    }
                }
            });
        }

        Self {
            side: Side::default(),
            image: ThreadProtocol::new(resize_tx, None),
            picker,
            side_tx,
            side_pending,
            side_generation,
            thumbnail_tx,
            thumbnail_epoch,
            thumbnails: HashMap::new(),
            thumbnail_order: VecDeque::new(),
            thumbnail_pending: HashSet::new(),
            viewport: Vec::new(),
            cache_capacity: 2,
        }
    }

    pub(crate) fn select(&mut self, path: &Path, is_dir: bool) {
        if self.side.path.as_deref() == Some(path) {
            return;
        }
        let generation = self.side_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.image.empty_protocol();
        if is_dir {
            self.side = Side {
                path: Some(path.to_path_buf()),
                body: folder_body(path),
                kind: Kind::Text,
                drag_icon: None,
            };
            return;
        }
        self.side = Side {
            path: Some(path.to_path_buf()),
            body: vec!["Loading preview…".into()],
            kind: Kind::Text,
            drag_icon: None,
        };
        let mut job = SideJob {
            generation,
            path: path.to_path_buf(),
        };
        loop {
            match self.side_tx.try_send(job) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) => {
                    job = returned;
                    let _ = self.side_pending.try_recv();
                }
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
    }

    pub(crate) fn clear_side(&mut self) {
        self.side_generation.fetch_add(1, Ordering::Relaxed);
        self.side = Side::default();
        self.image.empty_protocol();
    }

    pub(crate) fn set_cache_capacity(&mut self, visible: usize) {
        self.cache_capacity = visible.saturating_mul(2).max(2);
        self.evict();
    }

    pub(crate) fn request_viewport(&mut self, viewport: Vec<(PathBuf, Size)>) {
        if self.viewport != viewport {
            self.invalidate_grid();
            self.viewport = viewport;
        }
        self.set_cache_capacity(self.viewport.len());
        for (path, size) in self.viewport.clone() {
            self.request_thumbnail(&path, size);
        }
    }

    pub(crate) fn request_thumbnail(&mut self, path: &Path, size: Size) {
        if self
            .thumbnails
            .get(path)
            .is_some_and(|thumbnail| thumbnail.size == size)
        {
            self.touch(path);
            return;
        }
        if self.thumbnail_pending.contains(path) {
            return;
        }
        let path = path.to_path_buf();
        let epoch = self.thumbnail_epoch.load(Ordering::Relaxed);
        if self
            .thumbnail_tx
            .try_send((epoch, path.clone(), size))
            .is_ok()
        {
            self.thumbnail_pending.insert(path);
        }
    }

    pub(crate) fn thumbnail(&self, path: &Path, size: Size) -> Option<&Tile> {
        self.thumbnails
            .get(path)
            .filter(|thumbnail| thumbnail.size == size)
            .map(|thumbnail| &thumbnail.preview)
    }

    pub(crate) fn invalidate_grid(&mut self) {
        self.thumbnail_epoch.fetch_add(1, Ordering::Relaxed);
        self.thumbnail_pending.clear();
    }

    pub(crate) fn apply(&mut self, event: Event) -> bool {
        match event {
            Event::Side(generation, path, result)
                if self.side_generation.load(Ordering::Relaxed) == generation
                    && self.side.path.as_ref() == Some(&path) =>
            {
                match result {
                    Ok(Loaded::Image(image, drag_icon)) => {
                        self.side.kind = Kind::Image;
                        self.side.drag_icon = drag_icon;
                        self.image
                            .replace_protocol(self.picker.new_resize_protocol(image));
                        self.side.body.clear();
                    }
                    Ok(Loaded::Text(kind, body)) => {
                        self.side.kind = kind;
                        self.side.drag_icon = None;
                        self.side.body = body;
                    }
                    Err(error) => {
                        self.side.drag_icon = None;
                        self.side.body = vec![format!("Preview failed: {error}")];
                    }
                }
                true
            }
            Event::Side(_, _, _) => false,
            Event::Resize(Ok(resized)) => self.image.update_resized_protocol(resized),
            Event::Resize(Err(_)) => false,
            Event::Thumbnail(epoch, path, size, preview) => {
                if self.thumbnail_epoch.load(Ordering::Relaxed) != epoch {
                    return false;
                }
                self.thumbnail_pending.remove(&path);
                self.thumbnails
                    .insert(path.clone(), Thumbnail { size, preview });
                self.touch(&path);
                self.evict();
                true
            }
        }
    }

    fn touch(&mut self, path: &Path) {
        self.thumbnail_order.retain(|cached| cached != path);
        self.thumbnail_order.push_back(path.to_path_buf());
    }

    fn evict(&mut self) {
        while self.thumbnails.len() > self.cache_capacity {
            let Some(path) = self.thumbnail_order.pop_front() else {
                break;
            };
            self.thumbnails.remove(&path);
        }
    }
}

fn folder_body(path: &Path) -> Vec<String> {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(200)
        .map(|entry| {
            let path = entry.path();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            (
                is_dir,
                path,
                entry.file_name().to_string_lossy().into_owned(),
            )
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
    let mut body: Vec<_> = entries
        .into_iter()
        .map(|(is_dir, path, name)| format!("{}  {name}", icon_for(&path, is_dir)))
        .collect();
    if body.is_empty() {
        body.push("Empty folder".into());
    }
    body
}

#[derive(Clone, Copy)]
enum VisualKind {
    Image,
    Svg,
    Document,
    Video,
    Audio,
    Office,
    Font,
    ExtendedImage,
}

fn visual_kind(path: &Path) -> Option<VisualKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "bmp" | "gif" | "ico" | "jpeg" | "jpg" | "png" | "tif" | "tiff" | "webp" => {
            VisualKind::Image
        }
        "svg" | "svgz" => VisualKind::Svg,
        "pdf" | "ps" | "eps" | "dvi" | "djv" | "djvu" | "xps" | "oxps" | "ai" | "cb7" | "cbr"
        | "cbt" | "cbz" => VisualKind::Document,
        "3gp" | "asf" | "avi" | "flv" | "m2ts" | "m4v" | "mkv" | "mov" | "mp4" | "mpe" | "mpeg"
        | "mpg" | "mxf" | "ogv" | "ts" | "webm" | "wmv" => VisualKind::Video,
        "aac" | "aif" | "aiff" | "ape" | "flac" | "m4a" | "m4b" | "mp3" | "oga" | "ogg"
        | "opus" | "wav" | "wma" => VisualKind::Audio,
        "doc" | "docx" | "odp" | "ods" | "odt" | "pot" | "ppt" | "pptx" | "rtf" | "xls"
        | "xlsx" => VisualKind::Office,
        "otf" | "ttf" => VisualKind::Font,
        "avif" | "dds" | "exr" | "heic" | "heif" | "jxl" | "pbm" | "pgm" | "pnm" | "ppm"
        | "qoi" | "tga" => VisualKind::ExtendedImage,
        _ => return None,
    })
}

fn decode_image(
    path: &Path,
    target: Option<(Size, ratatui_image::FontSize)>,
    cancelled: impl Fn() -> bool,
) -> Result<image::DynamicImage, String> {
    let kind = visual_kind(path).ok_or_else(|| "unsupported preview format".to_owned())?;
    let (width, height) = target
        .map(|(size, font)| {
            (
                u32::from(size.width) * u32::from(font.width),
                u32::from(size.height) * u32::from(font.height),
            )
        })
        .unwrap_or((960, 720));
    if matches!(kind, VisualKind::Image) {
        let image = image::ImageReader::open(path)
            .map_err(|error| error.to_string())?
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .decode()
            .map_err(|error| error.to_string())?;
        return if cancelled() {
            Err("cancelled".into())
        } else {
            Ok(image.thumbnail(width.max(1), height.max(1)))
        };
    }
    external_thumbnail(path, kind, width.max(1), height.max(1), cancelled)
}

fn external_thumbnail(
    path: &Path,
    kind: VisualKind,
    width: u32,
    height: u32,
    cancelled: impl Fn() -> bool,
) -> Result<image::DynamicImage, String> {
    let output = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .map_err(|error| error.to_string())?;
    let output_path = output.path();
    let size = width.max(height).min(1600).to_string();
    let mut command;
    match kind {
        VisualKind::Svg => {
            command = Command::new("rsvg-convert");
            command
                .args(["--format", "png", "--keep-aspect-ratio", "--width"])
                .arg(width.to_string())
                .arg("--height")
                .arg(height.to_string())
                .arg("--output")
                .arg(output_path)
                .arg(path);
        }
        VisualKind::Document => {
            command = Command::new("evince-thumbnailer");
            command.arg("-s").arg(&size).arg(path).arg(output_path);
        }
        VisualKind::Video => {
            command = Command::new("ffmpegthumbnailer");
            command
                .arg("-i")
                .arg(path)
                .arg("-o")
                .arg(output_path)
                .arg("-s")
                .arg(&size)
                .args(["-c", "png", "-t", "10%"]);
        }
        VisualKind::Audio => {
            command = Command::new("ffmpeg");
            command
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(path)
                .args(["-t", "300", "-filter_complex"])
                .arg(format!(
                    "aformat=channel_layouts=mono,showwavespic=s={}x{}:colors=#8aa4ff",
                    width.min(1600),
                    height.min(1200)
                ))
                .args(["-frames:v", "1"])
                .arg(output_path);
        }
        VisualKind::Office => {
            command = Command::new("gsf-office-thumbnailer");
            command
                .arg("-i")
                .arg(path)
                .arg("-o")
                .arg(output_path)
                .arg("-s")
                .arg(&size);
        }
        VisualKind::Font => {
            command = Command::new("magick");
            command
                .args(["-size", &format!("{}x{}", width, height)])
                .args([
                    "-background",
                    "#09090b",
                    "-fill",
                    "#f4f4f5",
                    "-gravity",
                    "center",
                ])
                .arg("-font")
                .arg(path)
                .args(["-pointsize", &(height / 2).max(12).to_string(), "label:Aa"])
                .arg(output_path);
        }
        VisualKind::ExtendedImage => {
            command = Command::new("ffmpeg");
            command
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(path)
                .arg("-vf")
                .arg(format!(
                    "scale={width}:{height}:force_original_aspect_ratio=decrease"
                ))
                .args(["-frames:v", "1"])
                .arg(output_path);
        }
        VisualKind::Image => unreachable!(),
    }
    run(&mut command, cancelled)?;
    image::ImageReader::open(output_path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())
}

fn run(command: &mut Command, cancelled: impl Fn() -> bool) -> Result<(), String> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if cancelled() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancelled() {
                "cancelled".into()
            } else {
                "preview timed out".into()
            });
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut error = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut error);
                }
                return Err(error.trim().to_owned());
            }
            Ok(None) => thread::sleep(Duration::from_millis(8)),
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn load_side(path: &Path, cancelled: impl Fn() -> bool + Copy) -> Result<Loaded, String> {
    if visual_kind(path).is_some() {
        return decode_image(path, None, cancelled).map(|image| {
            let icon = drag_icon(&image);
            Loaded::Image(image, icon)
        });
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(96 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if cancelled() {
        return Err("cancelled".into());
    }
    if bytes.iter().take(8 * 1024).any(|byte| *byte == 0) {
        let description = Command::new("file")
            .args(["-b", "--"])
            .arg(path)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "Binary file".into());
        return Ok(Loaded::Text(
            Kind::Text,
            vec![
                description,
                String::new(),
                "Press Space for system preview".into(),
            ],
        ));
    }
    let kind = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx" | "markdown"
            )
        }) {
        Kind::Markdown
    } else {
        Kind::Text
    };
    let mut body: Vec<_> = String::from_utf8_lossy(&bytes)
        .lines()
        .take(400)
        .map(str::to_owned)
        .collect();
    if body.is_empty() {
        body.push("Empty file".into());
    }
    Ok(Loaded::Text(kind, body))
}

fn drag_icon(image: &image::DynamicImage) -> Option<DragIcon> {
    let image = image.thumbnail(224, 128);
    let (width, height) = (image.width(), image.height());
    let mut data = Cursor::new(Vec::new());
    image.write_to(&mut data, image::ImageFormat::Png).ok()?;
    Some(DragIcon {
        data: data.into_inner(),
        width,
        height,
    })
}

fn load_tile(
    path: &Path,
    size: Size,
    picker: &Picker,
    cancelled: impl Fn() -> bool + Copy,
) -> Tile {
    if visual_kind(path).is_some() {
        return decode_image(path, Some((size, picker.font_size())), cancelled)
            .and_then(|image| {
                if cancelled() {
                    return Err("cancelled".into());
                }
                picker
                    .new_protocol(image, size, Resize::Fit(None))
                    .map_err(|error| error.to_string())
            })
            .map(Tile::Image)
            .unwrap_or(Tile::Icon);
    }
    let Ok(file) = std::fs::File::open(path) else {
        return Tile::Icon;
    };
    let mut bytes = Vec::new();
    if file.take(4 * 1024).read_to_end(&mut bytes).is_err() || bytes.contains(&0) || cancelled() {
        return Tile::Icon;
    }
    let lines = String::from_utf8_lossy(&bytes)
        .lines()
        .map(|line| {
            line.chars()
                .map(|character| if character == '\t' { ' ' } else { character })
                .filter(|character| !character.is_control())
                .collect::<String>()
        })
        .map(|line| line.trim().to_owned())
        .filter(|line| {
            !line.is_empty()
                && !line
                    .chars()
                    .all(|character| character.is_whitespace() || "#=-_*".contains(character))
        })
        .take(size.height.max(1) as usize)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        Tile::Icon
    } else {
        Tile::Text(lines)
    }
}
