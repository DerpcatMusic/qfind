//! The Preview surfaces that need real interaction: an audio player with a
//! scrubbable waveform, and video.
//!
//! Everything here is a `gtk::Widget` built once for a file. The waveform is
//! drawn from decoded peak buckets, click and drag seek, and the playhead is
//! repainted from the playback clock on a tick — no polling of the filesystem, no
//! respawning a player per seek.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::audio::{self, Command, Player};
use crate::video;

/// Shared state. The gesture callbacks each hold an `Rc` of this, so a Preview
/// owns exactly one of them no matter how many controllers are attached.
struct Inner {
    duration: Cell<f64>,
    position: Cell<f64>,
    /// When playback actually started, so the playhead follows the wall clock
    /// instead of adding a fixed step per tick and drifting.
    anchor: Cell<Option<Instant>>,
    playing: Cell<bool>,
    /// A decoded track is loaded. Until it is, Space would flip a transport that
    /// has nothing to move.
    ready: Cell<bool>,
    scrubbing: Cell<bool>,
    /// Owned by the draw closure, which cannot borrow `Inner`. The cell mirrors
    /// `duration`/`position`/`playing` for the same reason.
    peaks_handle: Rc<RefCell<Vec<(f32, f32)>>>,
    duration_handle: Rc<Cell<f64>>,
    position_handle: Rc<Cell<f64>>,
    /// Where a drag started, so a delta becomes an absolute position.
    press_x: Cell<f64>,
    area: gtk::DrawingArea,
    player: Player,
    play_button: gtk::Button,
    clock: gtk::Label,
    /// The one line that says what is going on, including why nothing played.
    status: gtk::Label,
    tick: RefCell<Option<glib::SourceId>>,
}

impl Inner {
    fn new() -> Self {
        let status = gtk::Label::new(Some("Decoding…"));
        status.set_xalign(0.0);
        status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        status.add_css_class("dim-label");
        Self {
            duration: Cell::new(0.0),
            position: Cell::new(0.0),
            anchor: Cell::new(None),
            playing: Cell::new(false),
            ready: Cell::new(false),
            scrubbing: Cell::new(false),
            peaks_handle: Rc::new(RefCell::new(Vec::new())),
            duration_handle: Rc::new(Cell::new(0.0)),
            position_handle: Rc::new(Cell::new(0.0)),
            press_x: Cell::new(0.0),
            area: gtk::DrawingArea::new(),
            player: Player::new(),
            play_button: gtk::Button::new(),
            clock: gtk::Label::new(Some("0:00 / 0:00")),
            status,
            tick: RefCell::new(None),
        }
    }

    /// Seconds for a horizontal position, or `None` outside the waveform.
    fn seconds_at(&self, x: f64) -> Option<f64> {
        let width = f64::from(self.area.width());
        let total = self.duration.get();
        if width <= 1.0 || total <= 0.0 {
            return None;
        }
        Some((x.clamp(0.0, width) / width * total).clamp(0.0, total))
    }

    /// Where playback is right now, from elapsed time rather than tick counting.
    fn position_now(&self) -> f64 {
        let live = self
            .anchor
            .get()
            .map_or(0.0, |start| start.elapsed().as_secs_f64());
        (self.position.get() + live).min(self.duration.get())
    }

    fn seek_to_seconds(&self, seconds: f64) {
        let seconds = seconds.clamp(0.0, self.duration.get());
        self.position.set(seconds);
        // The new position becomes the base the clock counts on from.
        self.anchor.set(self.playing.get().then(Instant::now));
        self.clock.set_text(&format!(
            "{} / {}",
            audio::clock(seconds),
            audio::clock(self.duration.get())
        ));
        self.area.queue_draw();
        // Held back while the finger is down; the release commits one seek.
        if !self.scrubbing.get() {
            self.player.send(Command::Seek(seconds));
        }
    }

    /// Adopt a playing state, moving the icon to match. One place, so the icon
    /// can never disagree with the clock.
    fn set_playing(self: &Rc<Self>, playing: bool) {
        if playing == self.playing.get() {
            return;
        }
        if playing {
            self.anchor.set(Some(Instant::now()));
        } else {
            self.position.set(self.position_now());
            self.anchor.set(None);
        }
        self.playing.set(playing);
        self.player.set_playing(playing);
        self.play_button.set_icon_name(if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.start_clock();
    }

    fn toggle(self: &Rc<Self>) {
        if !self.ready.get() {
            return;
        }
        self.set_playing(!self.playing.get());
        self.player.send(if self.playing.get() {
            Command::Play
        } else {
            Command::Pause
        });
    }

    /// Repaint the playhead from the playback clock. The tick stops itself when
    /// nothing is playing, so an open Preview is not a permanent 60 ms timer.
    fn start_clock(self: &Rc<Self>) {
        if self.tick.borrow().is_some() {
            return;
        }
        let inner = Rc::clone(self);
        let source = glib::timeout_add_local(Duration::from_millis(60), move || {
            if let Some(message) = inner.player.take_error() {
                // The device refused the file, or there is no device. Say so
                // rather than leaving a paused playhead that looks broken.
                inner.set_playing(false);
                inner.player.send(Command::Pause);
                inner.status.set_text(&message);
            } else if inner.playing.get() {
                let next = inner.position_now();
                let total = inner.duration.get();
                if total > 0.0 && next >= total {
                    // Back to the start, with the clock agreeing with it.
                    inner.set_playing(false);
                    inner.position.set(0.0);
                    inner.clock.set_text(&format!(
                        "{} / {}",
                        audio::clock(0.0),
                        audio::clock(total)
                    ));
                    inner.area.queue_draw();
                } else {
                    inner.position.set(next);
                    inner.clock.set_text(&format!(
                        "{} / {}",
                        audio::clock(next),
                        audio::clock(total)
                    ));
                    inner.area.queue_draw();
                }
            }
            if inner.playing.get() || inner.scrubbing.get() {
                glib::ControlFlow::Continue
            } else {
                *inner.tick.borrow_mut() = None;
                glib::ControlFlow::Break
            }
        });
        *self.tick.borrow_mut() = Some(source);
    }

    fn draw_waveform(&self) {
        let area = self.area.clone();
        let peaks = Rc::clone(&self.peaks_handle);
        let position = Rc::clone(&self.position_handle);
        let duration = Rc::clone(&self.duration_handle);
        area.set_draw_func(move |_, cr, width, height| {
            let (width, height) = (f64::from(width), f64::from(height));
            let peaks = peaks.borrow();
            if peaks.is_empty() || duration.get() <= 0.0 {
                cr.set_source_rgba(0.55, 0.58, 0.64, 0.9);
                cr.move_to(8.0, height / 2.0);
                let _ = cr.show_text("no waveform");
                return;
            }
            let mid = height / 2.0;
            let step = width / peaks.len() as f64;
            let played = (position.get() / duration.get() * width).clamp(0.0, width);
            // One pass over pre-computed min/max buckets, so a 2 048-bucket
            // strip costs the same as a 20-bucket one and the playhead is free.
            for (index, (lo, hi)) in peaks.iter().enumerate() {
                let x = index as f64 * step;
                if x > width {
                    break;
                }
                let (lo, hi) = (f64::from(*lo), f64::from(*hi));
                let top = mid - hi * mid * 0.92;
                let bottom = mid - lo * mid * 0.92;
                if x + step <= played {
                    cr.set_source_rgba(0.36, 0.62, 0.96, 1.0);
                } else {
                    cr.set_source_rgba(0.62, 0.66, 0.74, 0.85);
                }
                cr.rectangle(x, top, (step + 0.5).max(1.0), (bottom - top).max(1.0));
            }
            let _ = cr.fill();
            cr.set_source_rgba(0.95, 0.36, 0.36, 1.0);
            cr.rectangle(played - 1.0, 0.0, 2.0, height);
            let _ = cr.fill();
        });
    }

    fn wire_input(self: &Rc<Self>) {
        let press = gtk::GestureClick::new();
        press.set_button(gdk::BUTTON_PRIMARY);
        {
            let inner = Rc::clone(self);
            press.connect_pressed(move |_, _, x, _| {
                inner.scrubbing.set(true);
                inner.press_x.set(x);
                if let Some(seconds) = inner.seconds_at(x) {
                    inner.seek_to_seconds(seconds);
                }
            });
        }
        {
            let inner = Rc::clone(self);
            press.connect_released(move |_, _, _, _| inner.commit_scrub());
        }
        self.area.add_controller(press);

        // Dragging scrubs. Without this, seeking a long track meant clicking,
        // waiting for the position to update, and clicking again.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        {
            let inner = Rc::clone(self);
            drag.connect_drag_begin(move |_, x, _| {
                inner.scrubbing.set(true);
                inner.press_x.set(x);
                if let Some(seconds) = inner.seconds_at(x) {
                    inner.seek_to_seconds(seconds);
                }
            });
        }
        {
            let inner = Rc::clone(self);
            // `pick` wants widget-relative fractions; the gesture gives us a
            // delta in surface pixels.
            drag.connect_drag_update(move |gesture, dx, dy| {
                let Some(area) = gesture.widget().and_downcast::<gtk::DrawingArea>() else {
                    return;
                };
                let start = inner.press_x.get();
                let x = (start + dx).clamp(0.0, f64::from(area.width().max(1)));
                let _ = dy;
                if let Some(seconds) = inner.seconds_at(x) {
                    inner.seek_to_seconds(seconds);
                }
            });
        }
        {
            let inner = Rc::clone(self);
            drag.connect_drag_end(move |_, _, _| inner.commit_scrub());
        }
        self.area.add_controller(drag);

        // Wheel over the waveform nudges the playhead, like every media app.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let inner = Rc::clone(self);
            scroll.connect_scroll(move |_, _dx, dy| {
                if dy.abs() < f64::EPSILON || inner.duration.get() <= 0.0 {
                    return glib::Propagation::Proceed;
                }
                let step = inner.duration.get() * 0.02;
                let next =
                    (inner.position.get() - dy.signum() * step).clamp(0.0, inner.duration.get());
                inner.seek_to_seconds(next);
                glib::Propagation::Stop
            });
        }
        self.area.add_controller(scroll);

        let play = self.play_button.clone();
        let inner = Rc::clone(self);
        play.connect_clicked(move |_| inner.toggle());

        // Space toggles, when the Preview has the focus.
        let keys = gtk::EventControllerKey::new();
        {
            let inner = Rc::clone(self);
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::space || key == gdk::Key::KP_Space {
                    inner.toggle();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        self.area.set_focusable(true);
        self.area.add_controller(keys);
    }

    /// Commit one seek when the finger lifts, so a drag does not restart the
    /// stream a hundred times.
    fn commit_scrub(&self) {
        if !self.scrubbing.replace(false) {
            return;
        }
        self.player.send(Command::Seek(self.position.get()));
    }
}

/// A Preview surface for an audio file: waveform, transport, scrubbing.
///
/// `play` opens it playing. Space is a request to *hear* the file, so the window
/// Space opens does; the Inspector's copy does not, or the track plays twice.
pub fn audio_preview(path: &Path, play: bool) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let heading = gtk::Label::new(Some(
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_str(),
    ));
    heading.set_xalign(0.0);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    heading.add_css_class("heading");
    root.append(&heading);

    let inner = Rc::new(Inner::new());
    inner
        .play_button
        .set_icon_name("media-playback-start-symbolic");
    inner
        .play_button
        .set_tooltip_text(Some("Play / pause (Space)"));
    inner.play_button.set_sensitive(false);
    inner.clock.add_css_class("dim-label");
    inner.area.set_content_width(560);
    inner.area.set_content_height(150);
    inner.area.set_vexpand(false);
    inner.area.set_hexpand(true);

    let detail = gtk::Label::new(Some(""));
    detail.add_css_class("dim-label");
    detail.set_hexpand(true);
    detail.set_xalign(1.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    transport.append(&inner.play_button);
    transport.append(&inner.clock);
    transport.append(&detail);
    root.append(&transport);
    root.append(&inner.status);
    root.append(&inner.area);

    let hint = gtk::Label::new(Some("Click or drag the waveform to seek · wheel to nudge"));
    hint.add_css_class("dim-label");
    hint.set_margin_top(2);
    root.append(&hint);

    inner.draw_waveform();
    inner.wire_input();

    // Decode on a worker: a 90-minute FLAC must not stall the window.
    let path = path.to_path_buf();
    let area = inner.area.clone();
    let status = inner.status.clone();
    let detail = detail.clone();
    glib::MainContext::default().spawn_local(async move {
        let decoded = gio::spawn_blocking(move || audio::decode(&path)).await;
        match decoded {
            Ok(Ok(track)) => {
                // A long file is capped in memory, and saying so beats a
                // waveform that stops for no visible reason.
                let length = if track.clipped {
                    format!("first {}", track.length)
                } else {
                    track.length.clone()
                };
                let summary = track.detail.clone();
                status.set_text(&format!("{length} · click or drag to seek"));
                detail.set_text(&summary);
                *inner.peaks_handle.borrow_mut() = track.peaks.clone();
                inner.duration.set(track.duration());
                inner.duration_handle.set(track.duration());
                inner.clock.set_text(&format!("0:00 / {length}"));
                inner.play_button.set_sensitive(true);
                inner.ready.set(true);
                inner.player.send(Command::Load(track));
                area.queue_draw();
                // Space means "hear it": the Preview opens playing, and a
                // machine with no output says why instead of sitting silent.
                if play {
                    inner.set_playing(true);
                    inner.player.send(Command::Play);
                }
            }
            Ok(Err(error)) => status.set_text(&format!("Cannot play this file: {error}")),
            Err(_) => status.set_text("The decoder failed"),
        }
    });

    root.upcast()
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

/// Is this something the video surface can play?
#[must_use]
pub fn is_video(path: &Path, content_type: &str) -> bool {
    if content_type.starts_with("video/") {
        return true;
    }
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "mp4"
                | "m4v"
                | "mkv"
                | "webm"
                | "mov"
                | "avi"
                | "mpg"
                | "mpeg"
                | "wmv"
                | "flv"
                | "ogv"
                | "3gp"
                | "ts"
                | "m2ts"
        )
    )
}

/// In-window video with a real transport: picture, scrub bar, clock.
///
/// `play` starts it moving. The Inspector's copy stays still, so one file is
/// never decoded twice at once.
///
/// Opening a file blocks until the pipeline has prerolled, which on a local file
/// is a few milliseconds and on a slow disk is a status line that says
/// `Loading…` instead of a frozen window.
pub fn video_preview(path: &Path, play: bool) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_size_request(640, 360);
    picture.set_vexpand(true);
    picture.set_hexpand(true);
    root.append(&picture);

    let status = gtk::Label::new(Some("Loading…"));
    status.set_xalign(0.0);
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    status.add_css_class("dim-label");
    root.append(&status);

    let play_button = gtk::Button::new();
    play_button.set_icon_name("media-playback-pause-symbolic");
    play_button.set_tooltip_text(Some("Play / pause (Space)"));
    let clock = gtk::Label::new(Some("0:00 / 0:00"));
    clock.add_css_class("dim-label");
    let detail = gtk::Label::new(Some(""));
    detail.add_css_class("dim-label");
    detail.set_hexpand(true);
    detail.set_xalign(1.0);
    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    transport.append(&play_button);
    transport.append(&clock);
    transport.append(&detail);
    root.append(&transport);

    // The scrub bar shows the pipeline's own position, so it agrees with what
    // is on screen instead of counting forward on a timer.
    let bar = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    bar.set_draw_value(false);
    bar.set_hexpand(true);
    root.append(&bar);

    let hint = gtk::Label::new(Some("Drag the bar to seek · Enter opens the system player"));
    hint.add_css_class("dim-label");
    hint.set_margin_top(2);
    root.append(&hint);

    let surface = video::Surface {
        picture,
        bar,
        clock,
        play_button,
        status,
    };
    video::attach(path, &surface, play);
    root.upcast()
}
