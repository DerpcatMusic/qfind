//! Video Preview: GStreamer plays the file, the GTK main thread paints it.
//!
//! GTK 4.12's `GtkVideo` shows a file and exposes no transport, so a scrub bar
//! wired to nothing would be worse than no scrub bar at all. `playbin` decodes
//! "whatever the file is" and gives us play, pause, seek and position for free;
//! an `appsink` hands the main thread RGB frames to upload as a texture.
//!
//! Frames are requested at one fixed size: GStreamer letterboxes whatever does
//! not match, so the widget never resizes and never has to letterbox itself.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as app;
use gstreamer_video as video;
use gtk::gdk;
use gtk::prelude::*;

/// The size every frame is delivered at. A preview does not need 4K, and a
/// fixed size keeps the frame path free of per-frame resizing.
const WIDTH: i32 = 640;
const HEIGHT: i32 = 360;

/// `format=RGB` is three bytes per pixel, no padding, no alpha.
const RGB_BYTES: usize = 3;

/// How long to wait for the pipeline to preroll before reporting a length. A
/// remote or network file can be slow; the Preview must not hang on it.
const PREROLL_TIMEOUT: Duration = Duration::from_secs(10);

/// One decoded frame on its way to a texture. Three bytes per pixel, rows
/// packed, which is exactly what `gdk::MemoryTexture` wants.
pub struct Frame {
    pub width: i32,
    pub height: i32,
    pub rgb: Vec<u8>,
}

impl Frame {
    /// Upload for the GTK main thread. `None` if the frame is not shaped like
    /// the buffer claims, which would otherwise be an out-of-bounds read.
    #[must_use]
    pub fn texture(&self) -> Option<gdk::Texture> {
        let stride = (self.width as usize).checked_mul(RGB_BYTES)?;
        if self.width <= 0 || self.height <= 0 || self.rgb.len() < stride * self.height as usize {
            return None;
        }
        let bytes = gtk::glib::Bytes::from_owned(self.rgb.clone());
        Some(
            gdk::MemoryTexture::new(
                self.width,
                self.height,
                gdk::MemoryFormat::R8g8b8,
                &bytes,
                stride,
            )
            .upcast(),
        )
    }
}

/// A playing video, with real transport.
///
/// Every method here is safe to call from the GTK main thread; the pipeline
/// runs on its own streaming threads and hands frames back over a channel.
pub struct Player {
    pipeline: gst::Element,
    frames: Receiver<Frame>,
    duration: Cell<f64>,
    playing: Cell<bool>,
    /// Why the pipeline gave up, drained by the UI.
    error: Arc<Mutex<Option<String>>>,
    /// Set from the bus watch. An atomic, because the player comes back to the
    /// main thread from a worker.
    finished: Arc<AtomicBool>,
}

impl Player {
    /// Open `path` and start playing it.
    ///
    /// Fails with a reason the Preview can show: no GStreamer, an unplayable
    /// file, or a missing decoder plugin.
    pub fn new(path: &Path) -> Result<Self, String> {
        gst::init().map_err(|error| error.to_string())?;
        let uri = gtk::gio::File::for_path(path).uri().to_string();

        // `playbin` is the one element that plays whatever the file happens to
        // be, so there is no format list to maintain here.
        let pipeline = gst::ElementFactory::make("playbin")
            .build()
            .map_err(|error| format!("no video decoder here: {error}"))?;
        let (tx, frames) = sync_channel(1);
        pipeline.set_property("video-sink", sink(&tx));

        let error = Arc::new(Mutex::new(None));
        let finished = Arc::new(AtomicBool::new(false));
        pipeline.set_property("uri", &uri);
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| error.to_string())?;
        // Preroll first: a pipeline that is still opening has no duration and
        // no seekable range, and a scrub bar that jumps is worse than a wait.
        let timeout = gst::ClockTime::from_nseconds(PREROLL_TIMEOUT.as_nanos() as u64);
        let (change, _, _) = pipeline.state(Some(timeout));
        change.map_err(|error| error.to_string())?;

        let player = Self {
            duration: Cell::new(seconds(pipeline.query_duration())),
            pipeline,
            frames,
            playing: Cell::new(true),
            error,
            finished,
        };
        if let Some(message) = player.take_error() {
            return Err(message);
        }
        Ok(player)
    }

    pub fn duration(&self) -> f64 {
        self.duration.get()
    }

    /// Where the pipeline actually is, which is the truth the scrub bar shows.
    #[must_use]
    pub fn position(&self) -> f64 {
        seconds(self.pipeline.query_position())
    }

    pub fn is_playing(&self) -> bool {
        self.playing.get()
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.set(playing);
        let _ = self.pipeline.set_state(if playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
    }

    /// Jump to `seconds`. Flushing, so a paused pipeline moves too instead of
    /// waiting for playback to resume.
    pub fn seek(&self, seconds: f64) {
        let target = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        let _ = self
            .pipeline
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target);
    }

    /// The newest frame, if one landed since the last call. Old ones are
    /// dropped at the sink, so this never queues up.
    pub fn take_frame(&self) -> Option<Frame> {
        self.frames.try_recv().ok()
    }

    /// Watch the bus for failure and end-of-file.
    ///
    /// Must run on the thread that owns the main context, which is why it is
    /// not part of `new`: the pipeline is built on a worker. The returned guard
    /// has to be kept or the watch is removed again, so the caller stores it.
    pub fn watch(&self) -> Result<gst::bus::BusWatchGuard, String> {
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| "the video pipeline has no bus".to_owned())?;
        let slot = Arc::clone(&self.error);
        let done = Arc::clone(&self.finished);
        bus.add_watch_local(move |_, message| match message.view() {
            gst::MessageView::Error(error) => {
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(error.error().to_string());
                }
                gtk::glib::ControlFlow::Break
            }
            gst::MessageView::Eos(_) => {
                done.store(true, Ordering::Relaxed);
                gtk::glib::ControlFlow::Break
            }
            _ => gtk::glib::ControlFlow::Continue,
        })
        .map_err(|error| error.to_string())
    }

    /// Take the pipeline's failure, so the UI can say why there is no picture.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Take the end-of-file flag, which the UI turns into a stopped transport.
    pub fn take_finished(&self) -> bool {
        self.finished.swap(false, Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Seconds for a stream time, or `0.0` while the pipeline has not decided.
fn seconds(time: Option<gst::ClockTime>) -> f64 {
    time.map_or(0.0, |time| time.seconds_f64())
}

/// An appsink that converts to packed RGB and posts frames to `tx`.
///
/// One dropped slot is deliberate: a preview only ever shows the newest frame,
/// so a slow UI must drop frames rather than stall the pipeline.
fn sink(tx: &std::sync::mpsc::SyncSender<Frame>) -> app::AppSink {
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGB")
        .field("width", WIDTH)
        .field("height", HEIGHT)
        // Square pixels, so a portrait clip is pillarboxed by GStreamer rather
        // than stretched by the widget.
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();
    let tx = tx.clone();
    let sink = app::AppSink::builder()
        .caps(&caps)
        .max_buffers(1)
        .drop(true)
        .sync(true)
        .build();
    sink.set_callbacks(
        app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let frame = frame(sink)?;
                match tx.try_send(frame) {
                    // A pending frame is one too old to be worth showing;
                    // dropping it keeps playback at full speed.
                    Ok(()) | Err(TrySendError::Full(_)) => Ok(gst::FlowSuccess::Ok),
                    Err(TrySendError::Disconnected(_)) => Err(gst::FlowError::Flushing),
                }
            })
            .build(),
    );
    sink
}

/// One sample as a tightly packed RGB frame.
///
/// The converter picks the row stride, and it is not always `width * 3`, so the
/// rows are repacked here rather than handing GTK a buffer it would read at the
/// wrong offsets.
fn frame(sink: &app::AppSink) -> Result<Frame, gst::FlowError> {
    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
    let caps = sample.caps().ok_or(gst::FlowError::Error)?;
    let info = video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::Error)?;
    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
    let frame = video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
        .map_err(|_| gst::FlowError::Error)?;
    let width = i32::try_from(info.width()).map_err(|_| gst::FlowError::Error)?;
    let height = i32::try_from(info.height()).map_err(|_| gst::FlowError::Error)?;
    let plane = frame.plane_data(0).map_err(|_| gst::FlowError::Error)?;
    let tight = (width as usize) * RGB_BYTES;
    let stride = usize::try_from(info.stride().first().copied().unwrap_or(0))
        .map_err(|_| gst::FlowError::Error)?;
    let stride = if stride >= tight { stride } else { tight };
    let mut rgb = Vec::with_capacity(tight * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        // A short plane means a truncated frame; keep what is there rather than
        // reading past the end, and let the texture check reject it.
        let end = (start + tight).min(plane.len());
        if start >= end {
            break;
        }
        rgb.extend_from_slice(&plane[start..end]);
    }
    Ok(Frame { width, height, rgb })
}

/// The widgets a video Preview is made of. The caller builds them so the
/// layout stays with the other Preview surfaces.
pub struct Surface {
    pub picture: gtk::Picture,
    pub bar: gtk::Scale,
    pub clock: gtk::Label,
    pub play_button: gtk::Button,
    pub status: gtk::Label,
}

/// The transport's own state, mirrored into the widgets.
struct State {
    surface: Surface,
    /// Dropping the guard uninstalls the bus watch, so the Preview owns it.
    _watch: RefCell<Option<gst::bus::BusWatchGuard>>,
    playing: Cell<bool>,
    scrubbing: Cell<bool>,
    /// Set while the widgets are being written from the pipeline, so the change
    /// handler does not treat the value it just read as a request to seek.
    updating: Cell<bool>,
    position: Cell<f64>,
    duration: Cell<f64>,
}

/// Wire a video Preview: open the file, run the transport, paint frames.
///
/// `play` starts it moving; the Inspector's copy stays on the first frame.
pub fn attach(path: &Path, surface: &Surface, play: bool) {
    let state = Rc::new(State {
        surface: Surface {
            picture: surface.picture.clone(),
            bar: surface.bar.clone(),
            clock: surface.clock.clone(),
            play_button: surface.play_button.clone(),
            status: surface.status.clone(),
        },
        _watch: RefCell::new(None),
        playing: Cell::new(false),
        scrubbing: Cell::new(false),
        updating: Cell::new(false),
        position: Cell::new(0.0),
        duration: Cell::new(0.0),
    });

    // Opening prerolls the pipeline, which blocks. On a local file that is a few
    // milliseconds; on a slow or network one it must not block the main thread.
    // Opening the file blocks, so it happens on a worker; the Preview's own
    // state is single-threaded and is only touched on the main context.
    let target = path.to_path_buf();
    gtk::glib::MainContext::default().spawn_local(async move {
        match gtk::gio::spawn_blocking(move || Player::new(&target)).await {
            Ok(Ok(player)) => ready(state, player, play),
            Ok(Err(message)) => failed(state, message),
            Err(_) => failed(state, "the video worker stopped".to_owned()),
        }
    });
}

/// The pipeline opened. Show the length and start the transport.
fn ready(state: Rc<State>, player: Player, play: bool) {
    state.surface.play_button.set_sensitive(true);
    // The watch belongs on the main thread, which is where we are now, and the
    // guard is kept or the watch is uninstalled straight away.
    match player.watch() {
        Ok(watch) => *state._watch.borrow_mut() = Some(watch),
        Err(message) => {
            failed(state, message);
            return;
        }
    }
    let player = Rc::new(player);
    wire(&state, &player);
    state.set_playing(play, &player);
    state.show_clock(0.0, player.duration());
    state.surface.status.set_text(&format!(
        "{} · drag to seek",
        crate::audio::clock(player.duration())
    ));

    let ticking = Rc::clone(&state);
    let pipeline = Rc::clone(&player);
    gtk::glib::timeout_add_local(Duration::from_millis(60), move || {
        if !tick(&ticking, &pipeline, play) {
            return gtk::glib::ControlFlow::Break;
        }
        gtk::glib::ControlFlow::Continue
    });
}

/// One tick: drain frames, follow the position, and notice the end.
///
/// The first frame is drawn even when nothing is playing, so a still Preview
/// still shows the video instead of an empty box.
fn tick(state: &Rc<State>, player: &Rc<Player>, play: bool) -> bool {
    let mut alive = true;
    let mut shown = true;
    if let Some(frame) = player.take_frame() {
        match frame.texture() {
            Some(texture) => state.surface.picture.set_paintable(Some(&texture)),
            None => shown = false,
        }
    } else {
        shown = false;
    }
    if let Some(message) = player.take_error() {
        state.surface.status.set_text(&message);
        state.set_playing(false, player);
        alive = false;
    }
    if player.take_finished() {
        state.set_playing(false, player);
        state.seek_bar(0.0);
        state.show_clock(0.0, state.duration.get());
        alive = false;
    }
    // While a press is down the bar belongs to the pointer: writing the
    // pipeline's position into it would undo the seek being aimed at.
    if player.is_playing() && !state.scrubbing.get() {
        let position = player.position();
        state.seek_bar(position);
        state.show_clock(position, state.duration.get());
    }
    alive || state.scrubbing.get() || (play && !shown)
}

/// The file could not be opened, so there is no transport to run.
fn failed(state: Rc<State>, message: String) {
    state.surface.status.set_text(&message);
    state.surface.play_button.set_sensitive(false);
    state.surface.bar.set_sensitive(false);
}

impl State {
    /// One place where the button, the clock, and the pipeline agree.
    fn set_playing(&self, playing: bool, player: &Player) {
        if playing == self.playing.replace(playing) {
            return;
        }
        player.set_playing(playing);
        self.surface.play_button.set_icon_name(if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
    }

    fn show_clock(&self, position: f64, duration: f64) {
        self.surface.clock.set_text(&format!(
            "{} / {}",
            crate::audio::clock(position),
            crate::audio::clock(duration)
        ));
    }

    /// Move the bar without the change handler reading it as a seek request.
    fn seek_bar(&self, position: f64) {
        self.updating.set(true);
        self.surface.bar.set_value(position);
        self.updating.set(false);
    }
}

/// Attach the transport to the widgets.
fn wire(state: &Rc<State>, player: &Rc<Player>) {
    let total = player.duration();
    state.duration.set(total);
    if total > 0.0 {
        state.surface.bar.set_range(0.0, total);
    }

    // A click or a key on the bar seeks; a drag is held back until it ends, so
    // one sweep of the thumb is one seek.
    {
        let inner = Rc::clone(state);
        let pipeline = Rc::clone(player);
        state.surface.bar.connect_value_changed(move |bar| {
            let value = bar.value();
            inner.position.set(value);
            inner.show_clock(value, inner.duration.get());
            if !inner.updating.get() && !inner.scrubbing.get() {
                pipeline.seek(value);
            }
        });
    }
    // Press starts a drag that seeks once, at the end, instead of a seek per
    // pixel the thumb moves.
    let press = gtk::GestureClick::new();
    press.set_button(gdk::BUTTON_PRIMARY);
    {
        let inner = Rc::clone(state);
        press.connect_pressed(move |_, _, _, _| inner.scrubbing.set(true));
    }
    {
        let inner = Rc::clone(state);
        let pipeline = Rc::clone(player);
        press.connect_released(move |_, _, _, _| {
            inner.scrubbing.set(false);
            pipeline.seek(inner.surface.bar.value());
        });
    }
    state.surface.bar.add_controller(press);

    {
        let inner = Rc::clone(state);
        let pipeline = Rc::clone(player);
        state
            .surface
            .play_button
            .connect_clicked(move |_| inner.set_playing(!inner.playing.get(), &pipeline));
    }

    // Space toggles wherever the focus is inside the Preview.
    state.surface.picture.set_focusable(true);
    let keys = gtk::EventControllerKey::new();
    {
        let inner = Rc::clone(state);
        let pipeline = Rc::clone(player);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::space || key == gdk::Key::KP_Space {
                inner.set_playing(!inner.playing.get(), &pipeline);
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
    }
    state.surface.picture.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: i32, height: i32) -> Frame {
        Frame {
            width,
            height,
            rgb: vec![0x40; (width as usize) * (height as usize) * 3],
        }
    }

    #[test]
    fn a_frame_uploads_as_a_texture() {
        assert!(frame(WIDTH, HEIGHT).texture().is_some());
    }

    #[test]
    fn a_frame_that_underruns_its_buffer_is_refused_not_uploaded() {
        // A truncated buffer would otherwise be read out of bounds inside GTK.
        let mut short = frame(WIDTH, HEIGHT);
        short.rgb.truncate(16);
        assert!(
            short.texture().is_none(),
            "must refuse, not read past the end"
        );
        let mut bad = frame(WIDTH, HEIGHT);
        bad.width = -1;
        assert!(bad.texture().is_none());
    }

    // The pipeline itself needs a GLib main context, which the unit-test
    // harness already owns, so a real file is played in the smoke test rather
    // than here.
}
