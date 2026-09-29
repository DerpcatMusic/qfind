//! Audio Preview: decode, waveform, and a real transport.
//!
//! Decoding is pure Rust (symphonia), so a waveform exists for every format we
//! can decode without shelling out. Playback is in-process (cpal) with the whole
//! signal in memory, which is what makes scrubbing *real*: seeking is a buffer
//! offset into PCM we already hold, not a kill-and-respawn of an external player.
//! Nothing here touches the interface thread — decoding and playback happen on
//! worker threads and report back over a channel.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decoded audio, held in memory. Everything the transport needs is here.
pub struct Track {
    pub path: PathBuf,
    /// Interleaved samples in `-1.0..=1.0`.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    /// Min/max peak per bucket, for the waveform. Two floats per bucket.
    pub peaks: Vec<(f32, f32)>,
    /// Human-readable length, e.g. `3:07`.
    pub length: String,
    /// Bitrate or channel summary, e.g. `48 kHz · stereo`.
    pub detail: String,
    /// The file was longer than [`MAX_SAMPLES`], so this is only its head. The
    /// Preview says so rather than quietly showing half an album.
    pub clipped: bool,
}

impl Clone for Track {
    fn clone(&self) -> Self {
        // The playback thread never draws, so it has no use for the peaks.
        Self {
            path: self.path.clone(),
            samples: self.samples.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            peaks: Vec::new(),
            length: self.length.clone(),
            detail: self.detail.clone(),
            clipped: self.clipped,
        }
    }
}

/// The three numbers every timing question needs: how many frames, how fast,
/// and how many channels. The decoded track and the device-rendered copy of it
/// both answer from this, so the clock, the playhead, and seeking can never
/// disagree about where "one second in" is.
#[derive(Clone, Copy)]
struct Geometry {
    rate: u32,
    channels: u16,
    frames: usize,
}

impl Geometry {
    fn of(samples: &[f32], rate: u32, channels: u16) -> Self {
        let channels = channels.max(1);
        Self {
            rate,
            channels,
            frames: samples.len() / usize::from(channels),
        }
    }

    /// Seconds, from the frame count.
    fn duration(&self) -> f64 {
        if self.rate == 0 {
            return 0.0;
        }
        self.frames as f64 / f64::from(self.rate)
    }

    /// Frame index for a position in seconds, clamped to the signal.
    fn frame_at(&self, seconds: f64) -> usize {
        if self.rate == 0 {
            return 0;
        }
        let frame = (seconds.max(0.0) * f64::from(self.rate)).round();
        (frame as usize).min(self.frames)
    }

    /// Interleaved sample index of a frame.
    fn index_of(&self, frame: usize) -> usize {
        frame.min(self.frames) * usize::from(self.channels)
    }
}

impl Track {
    fn geometry(&self) -> Geometry {
        Geometry::of(&self.samples, self.sample_rate, self.channels)
    }

    /// Seconds, from the decoded sample count.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.geometry().duration()
    }
}

/// `m:ss`, or `h:mm:ss` past an hour.
#[must_use]
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3_600, (total % 3_600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Peek at the file to decide whether it is audio, and say so cheaply.
#[must_use]
pub fn is_audio(path: &Path, content_type: &str) -> bool {
    if content_type.starts_with("audio/") {
        return true;
    }
    if matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "wav"
                | "wave"
                | "mp3"
                | "flac"
                | "ogg"
                | "oga"
                | "opus"
                | "aiff"
                | "aif"
                | "aifc"
                | "m4a"
                | "aac"
                | "wma"
                | "ape"
                | "mpc"
                | "wvp"
                | "caf"
                | "mid"
                | "midi"
        )
    ) {
        return true;
    }
    false
}

/// The most PCM the Preview is willing to hold, as f32 samples.
///
/// Ten minutes of 48 kHz stereo is 115 million samples, about 230 MB. Playback
/// needs the signal in memory to make seeking a buffer offset, so this is a
/// real limit rather than a formality: without it a header claiming four
/// gigabytes is a request for sixteen gigabytes and an OOM kill.
const MAX_SAMPLES: usize = 48_000 * 2 * 600;

/// Decode `path` into memory, failing with a reason the UI can show.
pub fn decode(path: &Path) -> Result<Track, String> {
    // Opening a fifo for reading waits for a writer that may never arrive, and
    // a symlink to /dev/zero or /dev/urandom never ends. Both look like audio
    // to a user, so both are turned away before anything blocks.
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err("not a regular file".into()),
        Err(error) => return Err(format!("{error}")),
    }
    let file = std::fs::File::open(path).map_err(|error| format!("{error}"))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("{error}"))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .cloned()
        .ok_or_else(|| "no audio track".to_owned())?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| format!("{error}"))?;

    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut samples: Vec<f32> = Vec::new();
    let mut clipped = false;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // A truncated or slightly-wrong file still has a usable head.
            Err(symphonia::core::errors::Error::ResetRequired)
            | Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(error) => return Err(format!("{error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let frames = decoded.frames();
                if sample_rate == 0 {
                    sample_rate = spec.rate;
                    channels = spec.channels.count() as u16;
                    // Pre-size from the timestamp so a long track does not
                    // reallocate its buffer on every packet.
                    let frames = packet.ts as usize / usize::from(channels.max(1));
                    if frames > 0 {
                        samples.reserve((frames + 1) * usize::from(channels));
                    }
                }
                // A fresh buffer per packet: the spec is only known here, and
                // reusing one would need the codec's alignment, which the
                // public `SignalSpec` does not expose.
                let mut buffer = SampleBuffer::<f32>::new(frames as u64, spec);
                buffer.copy_interleaved_ref(decoded);
                // Stop rather than grow: a file that claims more than the cap
                // gives the user what fits and says nothing, which beats an
                // allocation the machine cannot satisfy.
                let room = MAX_SAMPLES.saturating_sub(samples.len());
                if buffer.samples().len() >= room {
                    samples.extend_from_slice(&buffer.samples()[..room]);
                    clipped = true;
                    break;
                }
                samples.extend_from_slice(buffer.samples());
            }
            // One undecodable frame is not worth failing the whole Preview.
            Err(symphonia::core::errors::Error::DecodeError(_)) => {}
            Err(error) => return Err(format!("{error}")),
        }
    }
    if samples.is_empty() || sample_rate == 0 {
        return Err("no decodable audio".into());
    }
    let peaks = envelope(&samples, channels, WAVEFORM_BUCKETS);
    let duration = samples.len() as f64 / f64::from(sample_rate) / f64::from(channels);
    let detail = format!(
        "{} kHz · {}",
        sample_rate / 1_000,
        if channels > 1 { "stereo" } else { "mono" }
    );
    Ok(Track {
        path: path.to_path_buf(),
        samples,
        sample_rate,
        channels,
        peaks,
        length: clock(duration),
        detail,
        clipped,
    })
}

/// How many peak buckets the waveform holds, whatever the track length. Fixed
/// so the drawing is the same cost for a 3-second clip and a 90-minute one.
const WAVEFORM_BUCKETS: usize = 2_048;

/// Min/max per bucket, which is what a waveform actually needs: RMS alone
/// renders a quiet intro and a loud chorus the same height.
fn envelope(samples: &[f32], channels: u16, buckets: usize) -> Vec<(f32, f32)> {
    if samples.is_empty() || channels == 0 {
        return Vec::new();
    }
    let frames = samples.len() / usize::from(channels);
    if frames == 0 {
        return Vec::new();
    }
    let per_bucket = (frames / buckets).max(1);
    let mut peaks = Vec::with_capacity(buckets);
    let mut base = 0;
    while base < frames && peaks.len() < buckets {
        let end = (base + per_bucket).min(frames);
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for frame in base..end {
            for channel in 0..usize::from(channels) {
                let value = samples[frame * usize::from(channels) + channel];
                lo = lo.min(value);
                hi = hi.max(value);
            }
        }
        peaks.push((lo, hi));
        base = end;
    }
    peaks
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// A command for the playback thread.
pub enum Command {
    /// Take ownership of a decoded track, replacing whatever was playing.
    Load(Track),
    Play,
    Pause,
    /// Resume from `seconds`, re-buffering the signal from that offset.
    Seek(f64),
    /// Drop the track and stop.
    Release,
}

/// Handle to the playback thread. Cheap to clone, safe to hold anywhere.
#[derive(Clone)]
pub struct Player {
    tx: Sender<Command>,
    /// Kept so a dropped `Player` is a clean shutdown: the thread exits on
    /// `Release` instead of sitting on a channel nobody writes to.
    handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    playing: Arc<AtomicBool>,
    /// Why nothing came out of the speakers. Silence is a bug; this is the reason.
    error: Arc<Mutex<Option<String>>>,
}

impl Player {
    /// Start a player. The output device is opened on first play, so a machine
    /// with no sound card still shows a waveform and a working scrubber.
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let error = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&error);
        let handle = std::thread::Builder::new()
            .name("megaman-audio".into())
            .spawn(move || playback_loop(rx, slot))
            .ok();
        Self {
            tx,
            handle: Arc::new(Mutex::new(handle)),
            playing: Arc::new(AtomicBool::new(false)),
            error,
        }
    }

    /// `Ok(true)` when the channel is still live.
    pub fn send(&self, command: Command) -> bool {
        self.tx.send(command).is_ok()
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Relaxed);
    }

    /// Take the last playback failure, so the UI can say *why* nothing played.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Release);
        if let Ok(mut slot) = self.handle.lock()
            && let Some(handle) = slot.take()
        {
            let _ = handle.join();
        }
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

/// The decoded track in the shape the output device actually wants.
///
/// A 44.1 kHz file on a 48 kHz card used to be refused outright. Now the signal
/// is resampled once here, so a seek is a buffer offset instead of a failed
/// playback, and a mono file plays on a stereo card.
struct Rendered {
    data: Arc<Vec<f32>>,
    geometry: Geometry,
}

/// Where playback currently is, for the UI's clock.
struct Position {
    track: Track,
    source: Geometry,
    /// Built on the first `Play`, once the device has said what it wants.
    rendered: Option<Rendered>,
    stream: Option<cpal::Stream>,
    started: Option<Instant>,
    /// The paused position, in source frames.
    frame: usize,
    playing: bool,
}

impl Position {
    fn new(track: Track) -> Self {
        let source = Geometry::of(&track.samples, track.sample_rate, track.channels);
        Self {
            track,
            source,
            rendered: None,
            stream: None,
            started: None,
            frame: 0,
            playing: false,
        }
    }

    /// Source frame for right now, which is what a pause has to remember.
    fn now(&self) -> usize {
        let elapsed = self
            .started
            .map_or(0.0, |start| start.elapsed().as_secs_f64());
        let frame = self.frame as f64 + elapsed * f64::from(self.source.rate);
        (frame.round() as usize).min(self.source.frames)
    }

    /// Start or restart output at the current frame. `Err` is the reason the
    /// Preview stayed silent, and the caller shows it.
    fn resume(&mut self) -> Result<(), String> {
        if self.rendered.is_none() {
            self.rendered = Some(render(&self.track)?);
        }
        // The rendered signal is shared, never copied: a seek hands the same
        // buffer a new start offset instead of moving tens of megabytes.
        let rendered = self.rendered.as_ref().expect("rendered just above");
        let start =
            rendered
                .geometry
                .index_of(frame_in(rendered.geometry, self.source, self.frame));
        self.stream = Some(start_playback(Arc::clone(&rendered.data), start)?);
        self.started = Some(Instant::now());
        self.playing = true;
        Ok(())
    }
}

/// A source frame in another signal's frame space. Both signals are linear in
/// time, so this multiply is the whole conversion.
fn frame_in(target: Geometry, source: Geometry, frame: usize) -> usize {
    let rate = u64::from(source.rate).max(1);
    ((frame as u64 * u64::from(target.rate)) / rate) as usize
}

fn report(error: &Mutex<Option<String>>, message: String) {
    if let Ok(mut slot) = error.lock() {
        *slot = Some(message);
    }
}

fn playback_loop(rx: Receiver<Command>, error: Arc<Mutex<Option<String>>>) {
    let mut position: Option<Position> = None;
    while let Ok(command) = rx.recv() {
        match command {
            Command::Release => {
                drop(position.take());
                return;
            }
            Command::Load(track) => position = Some(Position::new(track)),
            Command::Pause => {
                if let Some(current) = position.as_mut() {
                    if current.playing {
                        current.frame = current.now();
                    }
                    current.stream = None;
                    current.started = None;
                    current.playing = false;
                }
            }
            Command::Play => {
                let Some(current) = position.as_mut() else {
                    continue;
                };
                if current.playing {
                    continue;
                }
                if let Err(message) = current.resume() {
                    current.playing = false;
                    current.started = None;
                    report(&error, message);
                }
            }
            Command::Seek(seconds) => {
                let Some(current) = position.as_mut() else {
                    continue;
                };
                let was_playing = current.playing;
                current.stream = None;
                current.started = None;
                current.playing = false;
                current.frame = current.source.frame_at(seconds);
                if was_playing && let Err(message) = current.resume() {
                    report(&error, message);
                }
            }
        }
    }
}

/// The decoded track in the device's format, resampled and remixed once.
fn render(track: &Track) -> Result<Rendered, String> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device".to_owned())?;
    let supported = device
        .default_output_config()
        .map_err(|error| error.to_string())?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!(
            "this device wants {} samples, not f32",
            supported.sample_format()
        ));
    }
    let config = supported.config();
    Ok(conform(track, config.sample_rate.0, config.channels as u16))
}

/// Resample and remix interleaved PCM into the device's rate and channel count.
///
/// Linear interpolation and an average are all a Preview needs: the point is
/// that the file is audible on whatever the machine has, not that this is a
/// mastering resampler.
fn conform(track: &Track, rate: u32, channels: u16) -> Rendered {
    let source = track.geometry();
    let channels = channels.max(1);
    if rate == source.rate && channels == source.channels {
        return Rendered {
            data: Arc::new(track.samples.clone()),
            geometry: source,
        };
    }
    let frames = if source.rate == 0 || rate == 0 {
        0
    } else {
        ((source.frames as u64 * u64::from(rate)) / u64::from(source.rate)) as usize
    };
    let out_channels = usize::from(channels);
    let step = f64::from(source.rate) / f64::from(rate);
    let mut data = Vec::with_capacity(frames * out_channels);
    let mut left = vec![0.0f32; out_channels];
    let mut right = vec![0.0f32; out_channels];
    let mut mixed = vec![0.0f32; out_channels];
    for index in 0..frames {
        let exact = index as f64 * step;
        let low = (exact as usize).min(source.frames.saturating_sub(1));
        let high = (low + 1).min(source.frames.saturating_sub(1));
        let blend = (exact - exact.floor()) as f32;
        remix(&track.samples, source.channels, low, &mut left);
        remix(&track.samples, source.channels, high, &mut right);
        for (channel, out) in mixed.iter_mut().enumerate() {
            *out = left[channel] + (right[channel] - left[channel]) * blend;
        }
        data.extend_from_slice(&mixed);
    }
    Rendered {
        data: Arc::new(data),
        geometry: Geometry {
            rate,
            channels,
            frames,
        },
    }
}

/// One source frame in the destination channel layout. A matching layout
/// copies, anything else averages into every destination channel, which
/// upmixes mono and downmixes surround alike.
fn remix(samples: &[f32], source_channels: u16, frame: usize, out: &mut [f32]) {
    let source_channels = usize::from(source_channels.max(1));
    let base = frame * source_channels;
    if source_channels == out.len() {
        let end = (base + out.len()).min(samples.len());
        if base < end {
            out.copy_from_slice(&samples[base..end]);
        }
        return;
    }
    let window = &samples[base..(base + source_channels).min(samples.len())];
    let mean = if window.is_empty() {
        0.0
    } else {
        window.iter().sum::<f32>() / source_channels as f32
    };
    out.fill(mean);
}

/// A cpal stream that plays interleaved `f32` from `start` to the end.
///
/// The signal is shared rather than sliced, so seeking costs an offset, not a
/// copy of everything that comes after it.
fn start_playback(data: Arc<Vec<f32>>, start: usize) -> Result<cpal::Stream, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device".to_owned())?;
    let format = device
        .default_output_config()
        .map_err(|error| format!("{error}"))?;
    // cpal has no sized-buffer type; the signal lives in the closure and a
    // cursor walks it, which is all `f32` interleaved playback needs.
    let cursor = Cell::new(start);
    let stream = device
        .build_output_stream(
            &format.config(),
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut at = cursor.get();
                for slot in output.iter_mut() {
                    *slot = data.get(at).copied().unwrap_or(0.0);
                    at += 1;
                }
                cursor.set(at.min(data.len()));
            },
            move |_| {},
            // A short buffer keeps `Command::Seek` responsive: the stream is
            // dropped and rebuilt, so nothing has to drain.
            Some(Duration::from_millis(20)),
        )
        .map_err(|error| format!("{error}"))?;
    stream.play().map_err(|error| format!("{error}"))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A 16-bit stereo WAV with a quiet intro, a loud middle, and a quiet tail.
    fn tone_wav(path: &Path) {
        let (rate, seconds) = (8_000u32, 2.0f32);
        let count = (rate as f32 * seconds) as usize;
        let mut samples: Vec<i16> = Vec::with_capacity(count * 2);
        for index in 0..count {
            let t = index as f32 / rate as f32;
            let envelope = if t < 0.5 || t > 1.5 { 0.05 } else { 0.9 };
            let value = (i16::MAX as f32
                * envelope
                * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16;
            samples.push(value);
            samples.push(value);
        }
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data.len() as u32).to_le_bytes())
            .unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&rate.to_le_bytes()).unwrap();
        file.write_all(&(rate * 4).to_le_bytes()).unwrap();
        file.write_all(&4u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        file.write_all(&data).unwrap();
    }

    #[test]
    fn decodes_a_wav_with_a_usable_waveform() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        tone_wav(&path);
        let track = decode(&path).expect("a 16-bit stereo wav must decode");

        assert_eq!(track.sample_rate, 8_000);
        assert_eq!(track.channels, 2);
        assert!(
            (track.duration() - 2.0).abs() < 0.01,
            "{}",
            track.duration()
        );
        assert_eq!(track.length, "0:02");
        assert_eq!(track.detail, "8 kHz · stereo");

        // The envelope has to describe the quiet intro and the loud middle, or
        // the waveform is a flat line and scrubbing it is guesswork.
        assert!(!track.peaks.is_empty());
        let loud = track.peaks.iter().map(|(_, hi)| *hi).fold(0.0f32, f32::max);
        let quiet_start = track.peaks[2].1;
        assert!(
            loud > 0.8,
            "loud section should peak near full scale: {loud}"
        );
        assert!(quiet_start < 0.15, "intro should be quiet: {quiet_start}");
    }

    #[test]
    fn seeking_lands_on_the_requested_sample() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        tone_wav(&path);
        let track = decode(&path).unwrap();
        // Halfway is exactly half the frames, which is what makes a scrub feel
        // accurate instead of approximately-there.
        let halfway = track.geometry().frame_at(1.0);
        let frames = track.samples.len() / usize::from(track.channels);
        assert_eq!(frames, 16_000, "2 seconds at 8 kHz");
        assert!(halfway.abs_diff(frames / 2) < 4, "{halfway} of {frames}");
        let geometry = track.geometry();
        assert_eq!(geometry.frame_at(-5.0), 0, "clamped at the start");
        assert!(geometry.frame_at(999.0) <= frames, "clamped at the end");
    }

    #[test]
    fn a_device_format_that_does_not_match_is_resampled_not_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        tone_wav(&path);
        let track = decode(&path).unwrap();

        // The common case: 44.1 kHz stereo on a 48 kHz card.
        let fast = conform(&track, 16_000, 2);
        assert_eq!(fast.geometry.rate, 16_000);
        assert_eq!(fast.geometry.channels, 2);
        assert!(
            (fast.geometry.duration() - 2.0).abs() < 0.01,
            "{}",
            fast.geometry.duration()
        );
        assert_eq!(fast.data.len(), fast.geometry.frames * 2);
        // Resampling must not turn a signal into silence or a wall of noise.
        let peak = fast.data.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.8 && peak <= 1.0, "peak should survive: {peak}");

        // Mono out of stereo averages, so the layout halves but the frames stay.
        let mono = conform(&track, 8_000, 1);
        assert_eq!(mono.geometry.channels, 1);
        assert_eq!(
            mono.geometry.frames,
            track.samples.len() / usize::from(track.channels)
        );
        assert_eq!(mono.data.len(), mono.geometry.frames);

        // A matching format is the signal itself, untouched.
        let same = conform(&track, track.sample_rate, track.channels);
        assert_eq!(same.data.as_slice(), track.samples.as_slice());
    }

    #[test]
    fn frames_map_between_a_track_and_its_rendered_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        tone_wav(&path);
        let track = decode(&path).unwrap();
        let source = track.geometry();
        // 16 000 frames at 8 kHz is 2 s; at 16 kHz that is 32 000 frames.
        let target = Geometry {
            rate: 16_000,
            channels: 2,
            frames: 32_000,
        };
        assert_eq!(frame_in(target, source, 8_000), 16_000);
        assert_eq!(frame_in(target, source, 0), 0);
        // And back the other way, which is what a pause does.
        assert_eq!(frame_in(source, target, 16_000), 8_000);
    }

    #[test]
    fn clock_formats_short_and_long_tracks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(9.0), "0:09");
        assert_eq!(clock(187.0), "3:07");
        assert_eq!(clock(3_723.0), "1:02:03");
        assert_eq!(clock(-1.0), "0:00", "never negative");
    }

    #[test]
    fn a_non_audio_file_reports_why_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"not audio at all").unwrap();
        assert!(decode(&path).is_err(), "must fail cleanly");
        let missing = dir.path().join("gone.wav");
        assert!(decode(&missing).is_err());
    }
}

#[cfg(test)]
pub fn envelope_for_test(samples: &[f32], channels: usize) -> Vec<(f32, f32)> {
    envelope(samples, channels as u16, WAVEFORM_BUCKETS)
}

#[cfg(test)]
mod hostile {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::FileTypeExt;

    /// A Preview must not be able to hang the process. `open()` on a fifo waits
    /// for a writer that never arrives, so a `song.wav` that happens to be a
    /// fifo left the window on "Decoding…" forever and leaked a thread.
    #[test]
    fn a_fifo_never_hangs_the_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frozen.wav");
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        if !made {
            return; // no mkfifo here; nothing to prove
        }
        assert!(std::fs::metadata(&path).unwrap().file_type().is_fifo());

        let start = Instant::now();
        let outcome = decode(&path);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "decoding a fifo took {:?}",
            start.elapsed()
        );
        assert!(outcome.is_err(), "a fifo is not audio and must not decode");
    }

    /// A symlink to a character device is the same trap wearing another hat.
    #[test]
    fn a_device_is_not_audio() {
        assert!(decode(Path::new("/dev/zero")).is_err(), "reads never end");
        assert!(decode(Path::new("/dev/null")).is_err(), "no audio there");
    }

    /// A WAV header claiming four gigabytes must not turn into sixteen gigabytes
    /// of `f32`. The Preview takes what fits and says it was cut.
    #[test]
    fn a_header_that_lies_about_its_size_is_capped_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.wav");
        let mut header: Vec<u8> = Vec::new();
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes()); // PCM
        header.extend_from_slice(&2u16.to_le_bytes()); // stereo
        header.extend_from_slice(&44_100u32.to_le_bytes());
        header.extend_from_slice(&176_400u32.to_le_bytes());
        header.extend_from_slice(&4u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(u64::from(u32::MAX)).unwrap();
        drop(file);

        let track = decode(&path).expect("the head of the file is real audio");
        assert!(track.clipped, "the user has to be told it was cut short");
        assert!(
            track.samples.len() <= MAX_SAMPLES,
            "held {} samples, over the cap of {MAX_SAMPLES}",
            track.samples.len()
        );
        // And what it does hold is still a usable waveform.
        assert!(!track.peaks.is_empty());
    }
}
// Throw absurd values at the pure helpers. A panic here is a crash the user
// reaches by accident, not by malice.
#[cfg(test)]
mod edge {
    use super::*;

    #[test]
    fn clock_survives_nonsense() {
        for bad in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.0,
            1e300,
            -1e300,
        ] {
            let out = clock(bad);
            assert!(!out.is_empty());
            assert!(
                !out.contains("NaN") && !out.contains("inf"),
                "{bad} -> {out}"
            );
        }
    }

    #[test]
    fn envelope_survives_odd_shapes() {
        for (len, channels) in [(0usize, 1usize), (1, 1), (3, 2), (7, 3), (1, 64), (1024, 0)] {
            let samples = vec![0.5f32; len * channels.max(1)];
            let peaks = envelope_for_test(&samples, channels);
            for (lo, hi) in &peaks {
                assert!(
                    lo.is_finite() && hi.is_finite(),
                    "{len}x{channels}: {lo} {hi}"
                );
                assert!(lo <= hi);
            }
        }
    }

    #[test]
    fn conform_survives_zero_rate_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.wav");
        std::fs::write(&path, b"not a wav at all, just bytes").unwrap();
        // Must not panic, must not divide by zero into a NaN length.
        let _ = decode(&path);
    }
}
