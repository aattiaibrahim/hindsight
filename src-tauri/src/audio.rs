//! Audio capture for the replay buffer.
//!
//! Desktop audio is WASAPI loopback of the default output device; the mic is
//! the default (or chosen) input device, optionally downmixed to mono. Each
//! source is streamed to ffmpeg as raw f32 PCM over a localhost socket.
//!
//! WASAPI loopback delivers nothing while the PC is silent, so the writer paces
//! itself by the wall clock and fills gaps with silence. That keeps audio
//! locked to the video's timeline however long the buffer runs.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use std::collections::VecDeque;
use std::io::{ErrorKind, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Microphone volume as a multiplier (f32 bits), changed live from Settings.
static MIC_GAIN: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

/// Make the mic louder or quieter while recording (-20..+20 dB).
pub fn set_mic_gain_db(db: f64) {
    let g = 10f64.powf(db.clamp(-20.0, 20.0) / 20.0) as f32;
    MIC_GAIN.store(g.to_bits(), Ordering::Relaxed);
}

fn mic_gain() -> f32 {
    f32::from_bits(MIC_GAIN.load(Ordering::Relaxed))
}

/// Scale samples, clamped so a boost can't wrap around into noise.
fn apply_gain(samples: &mut [f32], g: f32) {
    if (g - 1.0).abs() > 1e-4 {
        for x in samples {
            *x = (*x * g).clamp(-1.0, 1.0);
        }
    }
}

pub enum Source {
    Desktop,
    /// Everything except Discord (falls back to `Desktop` on older Windows)
    DesktopNoDiscord,
    /// Only Discord
    Discord,
    Mic { device: String, mono: bool },
}

/// A running capture. Dropping it stops the device and the socket writer.
pub struct Capture {
    stop: Arc<AtomicBool>,
    pub rate: u32,
    pub channels: u16,
    pub url: String,
    pub device: String,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type Queue = Arc<Mutex<VecDeque<f32>>>;
/// Audio chunks with the moment Windows captured their first sample.
type Chunks = Arc<Mutex<VecDeque<(Instant, Vec<f32>)>>>;

/// Names of the current default output and input devices (to notice headset swaps).
pub fn default_devices() -> (Option<String>, Option<String>) {
    let h = cpal::default_host();
    (
        h.default_output_device().and_then(|d| d.name().ok()),
        h.default_input_device().and_then(|d| d.name().ok()),
    )
}

pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// `delay` seconds of silence are sent first, shifting this track later in time.
pub fn start(src: Source, delay: f64) -> Result<Capture, String> {
    let (stop, queue, rate, channels, device) = open(src, None)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    {
        let (stop, queue) = (stop.clone(), queue.clone());
        thread::spawn(move || pump(listener, queue, rate, channels as usize, delay, stop));
    }
    Ok(Capture { stop, rate, channels, url: format!("tcp://127.0.0.1:{port}"), device })
}

type Opened = (Arc<AtomicBool>, Queue, u32, u16, String);

/// Open the device on its own thread (cpal streams must stay on one thread).
/// With `chunks`, audio arrives timestamped there instead of in the queue.
fn open(src: Source, chunks: Option<Chunks>) -> Result<Opened, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
    let (tx, rx) = mpsc::channel::<Result<(u32, u16, String), String>>();

    let per_app = match &src {
        Source::DesktopNoDiscord => Some(crate::app_audio::Target::AllButDiscord),
        Source::Discord => Some(crate::app_audio::Target::Discord),
        _ => None,
    };
    if let Some(target) = per_app {
        match open_per_app(target, stop.clone(), queue.clone(), chunks.clone()) {
            Ok(name) => return Ok((stop, queue, crate::app_audio::RATE, crate::app_audio::CHANNELS, name)),
            // Older Windows: desktop audio still works, just with Discord in it.
            Err(e) if target == crate::app_audio::Target::AllButDiscord => {
                crate::log!("[rewind] desktop audio without Discord isn't available ({e}); using all desktop audio");
                return open(Source::Desktop, chunks);
            }
            Err(e) => return Err(e),
        }
    }

    // The cpal stream has to stay on the thread that made it.
    {
        let (stop, queue) = (stop.clone(), queue.clone());
        thread::spawn(move || match open_stream(&src, queue, chunks) {
            Ok((stream, rate, ch, name)) => {
                let _ = tx.send(Ok((rate, ch, name)));
                while !stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        });
    }
    let (rate, channels, device) = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "The audio device didn't respond.".to_string())??;
    Ok((stop, queue, rate, channels, device))
}

/// Per-app capture (see app_audio) feeding the same queue / chunks as cpal.
fn open_per_app(target: crate::app_audio::Target, stop: Arc<AtomicBool>, q: Queue, chunks: Option<Chunks>) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    let max = crate::app_audio::RATE as usize * crate::app_audio::CHANNELS as usize * 2;
    thread::Builder::new()
        .name("rewind-app-audio".into())
        .spawn(move || {
            crate::app_audio::run(target, stop, tx, move |at, samples| {
                if let Some(chunks) = &chunks {
                    let mut c = chunks.lock().unwrap();
                    c.push_back((at, samples));
                    while c.len() > 400 {
                        c.pop_front();
                    }
                } else {
                    let mut q = q.lock().unwrap();
                    q.extend(samples);
                    while q.len() > max {
                        q.pop_front();
                    }
                }
            })
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(6)).map_err(|_| "Per-app audio didn't respond.".to_string())??;
    Ok(match target {
        crate::app_audio::Target::AllButDiscord => "everything except Discord".into(),
        crate::app_audio::Target::Discord => "Discord".into(),
    })
}

fn open_stream(src: &Source, q: Queue, chunks: Option<Chunks>) -> Result<(cpal::Stream, u32, u16, String), String> {
    let host = cpal::default_host();
    let is_mic = matches!(src, Source::Mic { .. });
    let (dev, cfg, mono) = match src {
        Source::Desktop | Source::DesktopNoDiscord | Source::Discord => {
            let d = host.default_output_device().ok_or("No speakers or headphones found.")?;
            let c = d.default_output_config().map_err(|e| e.to_string())?;
            (d, c, false)
        }
        Source::Mic { device, mono } => {
            let chosen = if device.is_empty() {
                None
            } else {
                host.input_devices()
                    .ok()
                    .and_then(|mut it| it.find(|d| d.name().ok().as_deref() == Some(device.as_str())))
            };
            let d = chosen.or_else(|| host.default_input_device()).ok_or("No microphone found.")?;
            let c = d.default_input_config().map_err(|e| e.to_string())?;
            (d, c, *mono)
        }
    };
    let name = dev.name().unwrap_or_default();
    crate::log!("audio: {} from {name} ({} Hz, {} ch)", if is_mic { "mic" } else { "desktop" }, cfg.sample_rate().0, cfg.channels());
    let config: cpal::StreamConfig = cfg.config();
    let in_ch = config.channels as usize;
    let out_ch = if mono { 1 } else { config.channels };
    let rate = config.sample_rate.0;
    let max = rate as usize * out_ch as usize * 2; // never hold more than 2 s

    // On WASAPI, an input stream on an output device is loopback capture.
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build::<f32>(&dev, &config, in_ch, mono, is_mic, q, chunks, max),
        SampleFormat::I16 => build::<i16>(&dev, &config, in_ch, mono, is_mic, q, chunks, max),
        SampleFormat::I32 => build::<i32>(&dev, &config, in_ch, mono, is_mic, q, chunks, max),
        SampleFormat::U16 => build::<u16>(&dev, &config, in_ch, mono, is_mic, q, chunks, max),
        f => Err(format!("Unsupported audio format {f:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate, out_ch, name))
}

fn build<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    in_ch: usize,
    mono: bool,
    is_mic: bool,
    q: Queue,
    chunks: Option<Chunks>,
    max: usize,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    dev.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            if let Some(chunks) = &chunks {
                // WASAPI reports when this buffer was captured (QPC-based, the
                // same clock as the video). Convert that to our clock.
                let ts = info.timestamp();
                let delay = ts.callback.duration_since(&ts.capture).unwrap_or_default();
                let at = Instant::now().checked_sub(delay).unwrap_or_else(Instant::now);
                let mut samples: Vec<f32> = if mono && in_ch > 1 {
                    data.chunks(in_ch)
                        .map(|f| f.iter().map(|&x| f32::from_sample(x)).sum::<f32>() / in_ch as f32)
                        .collect()
                } else {
                    data.iter().map(|&x| f32::from_sample(x)).collect()
                };
                if is_mic {
                    apply_gain(&mut samples, mic_gain());
                }
                let mut c = chunks.lock().unwrap();
                c.push_back((at, samples));
                while c.len() > 400 {
                    c.pop_front(); // nobody is reading: don't grow forever
                }
                return;
            }
            let g = if is_mic { mic_gain() } else { 1.0 };
            let mut q = q.lock().unwrap();
            if mono && in_ch > 1 {
                for frame in data.chunks(in_ch) {
                    let sum: f32 = frame.iter().map(|&x| f32::from_sample(x)).sum();
                    q.push_back((sum / in_ch as f32 * g).clamp(-1.0, 1.0));
                }
            } else {
                q.extend(data.iter().map(|&x| (f32::from_sample(x) * g).clamp(-1.0, 1.0)));
            }
            while q.len() > max {
                q.pop_front();
            }
        },
        |e| crate::log!("audio stream error: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

/// Feed ffmpeg at exactly `rate` frames per second of wall time.
fn pump(listener: TcpListener, q: Queue, rate: u32, ch: usize, delay: f64, stop: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    let give_up = Instant::now() + Duration::from_secs(20);
    let mut sock = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() > give_up {
            return;
        }
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
            Err(_) => return,
        }
    };
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_nodelay(true);
    q.lock().unwrap().clear(); // start the timeline now

    // Sync offset: a silent lead-in pushes every later sample back by `delay`.
    let lead = vec![0u8; (rate as f64 * delay) as usize * ch * 4];
    if sock.write_all(&lead).is_err() {
        return;
    }
    let mut bytes: Vec<u8> = Vec::new();
    pace(&q, rate, ch, &stop, Instant::now(), |frames| {
        bytes.clear();
        for s in frames {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        sock.write_all(&bytes).is_ok()
    });
}

/// Emit exactly `rate` frames per second of wall time from `t0` on: sample k
/// always means time t0 + k/rate. Gaps (silent loopback) become zeros; if the
/// device runs ahead of the clock, the oldest audio is dropped to resync.
fn pace(q: &Queue, rate: u32, ch: usize, stop: &AtomicBool, t0: Instant, mut emit: impl FnMut(&[f32]) -> bool) {
    let cushion = rate as u64 / 10; // stay 100 ms behind real time to absorb device jitter
    let backlog = rate as usize * 3 / 10;
    let mut written: u64 = 0;
    let mut buf: Vec<f32> = Vec::with_capacity(rate as usize * ch / 10);
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(10));
        let due = (t0.elapsed().as_secs_f64() * rate as f64) as u64;
        if due <= written + cushion {
            continue;
        }
        let need = (due - cushion - written) as usize;
        buf.clear();
        {
            let mut q = q.lock().unwrap();
            let avail = q.len() / ch;
            if avail > need + backlog {
                let drop_frames = avail - need - cushion as usize;
                q.drain(..drop_frames * ch);
            }
            let take = need.min(q.len() / ch);
            buf.extend(q.drain(..take * ch));
            buf.resize(need * ch, 0.0); // silence
        }
        if !emit(&buf) {
            return;
        }
        written += need as u64;
    }
}

/// Audio kept in memory for the native engine. Frame k (counting from the
/// very first) is the sound at `start + k / rate` on the shared clock.
pub struct AudioRing {
    pub rate: u32,
    pub channels: u16,
    pub start: Instant,
    /// Absolute frame number of the oldest frame still held.
    base: u64,
    data: VecDeque<f32>,
    max_frames: usize,
}

impl AudioRing {
    fn push(&mut self, frames: &[f32]) {
        self.data.extend(frames.iter().copied());
        self.trim();
    }
    fn push_silence(&mut self, frames: usize) {
        self.data.resize(self.data.len() + frames * self.channels as usize, 0.0);
        self.trim();
    }
    fn trim(&mut self) {
        let ch = self.channels as usize;
        let excess = (self.data.len() / ch).saturating_sub(self.max_frames);
        if excess > 0 {
            self.data.drain(..excess * ch);
            self.base += excess as u64;
        }
    }
    /// Absolute frame number the next sample will get.
    fn head(&self) -> i64 {
        (self.base + (self.data.len() / self.channels as usize) as u64) as i64
    }
    fn frame_at(&self, t: Instant) -> i64 {
        if t >= self.start {
            ((t - self.start).as_secs_f64() * self.rate as f64).round() as i64
        } else {
            -(((self.start - t).as_secs_f64() * self.rate as f64).round() as i64)
        }
    }

    /// `frames` frames starting at time `from` (zeros where nothing is held).
    pub fn slice(&self, from: Instant, frames: usize) -> Vec<f32> {
        let ch = self.channels as usize;
        let mut out = vec![0f32; frames * ch];
        let first = if from >= self.start {
            ((from - self.start).as_secs_f64() * self.rate as f64).round() as i64
        } else {
            -(((self.start - from).as_secs_f64() * self.rate as f64).round() as i64)
        };
        let held_from = self.base as i64;
        let held_to = held_from + (self.data.len() / ch) as i64;
        for i in 0..frames as i64 {
            let abs = first + i;
            if abs >= held_from && abs < held_to {
                let src = ((abs - held_from) as usize) * ch;
                out[i as usize * ch..][..ch].copy_from_slice(&self.data.range(src..src + ch).copied().collect::<Vec<_>>());
            }
        }
        out
    }

    pub fn bytes(&self) -> usize {
        self.data.len() * 4
    }

    /// Time of the newest frame held.
    pub fn end_time(&self) -> Instant {
        let frames = self.base + (self.data.len() / self.channels as usize) as u64;
        self.start + Duration::from_secs_f64(frames as f64 / self.rate as f64)
    }
}

/// Capture into an in-memory ring (native engine) holding `seconds` of audio.
pub fn start_ring(src: Source, seconds: u32) -> Result<(Capture, Arc<Mutex<AudioRing>>), String> {
    let t0 = Instant::now();
    let chunks: Chunks = Arc::new(Mutex::new(VecDeque::new()));
    let (stop, _queue, rate, channels, device) = open(src, Some(chunks.clone()))?;
    let ring = Arc::new(Mutex::new(AudioRing {
        rate,
        channels,
        start: t0,
        base: 0,
        data: VecDeque::new(),
        max_frames: rate as usize * seconds as usize,
    }));
    {
        let (stop, ring) = (stop.clone(), ring.clone());
        thread::spawn(move || place(&chunks, &ring, &stop));
    }
    Ok((Capture { stop, rate, channels, url: String::new(), device }, ring))
}

/// Timestamp wobble smaller than this is treated as continuous audio (OBS
/// uses 70 ms). Device timestamps jitter by several ms, especially on USB
/// interfaces and at 44.1 kHz; treating that jitter as real gaps put silence
/// between chunks and cut overlaps, which sounded choppy.
const SMOOTH_MS: i64 = 50;
/// Start nudging audio back toward its timestamps when it has drifted this far.
const DRIFT_MS: f64 = 5.0;
/// With no audio arriving for this long, the device is idle (silent loopback).
const IDLE_AFTER: Duration = Duration::from_millis(1000);
/// While idle, keep the ring filled with silence up to this far behind "now".
const IDLE_LAG: Duration = Duration::from_millis(500);

/// Places captured chunks into the ring.
///
/// Back-to-back chunks are joined seamlessly: their timestamps only decide
/// where audio goes after a real break (the device was silent or restarted),
/// or when they disagree by more than `SMOOTH_MS`. The slow drift between the
/// sound card's clock and the system clock is corrected one frame at a time,
/// by stretching or squeezing a whole chunk by one frame with interpolation
/// (a 0.2% change over ~10 ms, far below hearing, and no click), so audio
/// stays locked to the video however long Hindsight runs.
struct Placer {
    ch: usize,
    smooth: i64,
    drift: f64,
    /// Chunks held back after a start or break until the true start time is
    /// known: some devices report packets 10-20 ms late, so the first one
    /// alone can't be trusted.
    pending: Vec<(i64, Vec<f32>)>,
    anchoring: bool,
    /// Recent (timestamp position − ring position) offsets, in frames.
    /// Timestamps are only ever late, so the smallest recent offset is the
    /// true one.
    recent: VecDeque<i64>,
    /// That smallest offset: how far the audio sits from where it belongs.
    err: f64,
    /// Chunks since the last one-frame correction (they're spaced out).
    since_nudge: u32,
}

/// At most one one-frame correction per this many chunks (~every 30 ms:
/// enough to follow up to ~700 ppm of clock drift, far more than real
/// sound cards have).
const NUDGE_EVERY: u32 = 3;
/// Chunks (~200 ms) looked at to find the true start time after a break.
const ANCHOR_CHUNKS: usize = 20;

impl Placer {
    fn new(rate: u32, ch: usize) -> Placer {
        Placer {
            ch,
            smooth: rate as i64 * SMOOTH_MS / 1000,
            drift: rate as f64 * DRIFT_MS / 1000.0,
            pending: Vec::new(),
            anchoring: true,
            recent: VecDeque::new(),
            err: 0.0,
            since_nudge: 0,
        }
    }

    /// Add one chunk captured at ring frame `idx`.
    fn place(&mut self, r: &mut AudioRing, idx: i64, samples: &[f32]) {
        let ch = self.ch;
        let frames = (samples.len() / ch) as i64;
        if frames == 0 {
            return;
        }
        if self.anchoring {
            // A jump inside the held chunks is another break: settle what we have first.
            if let Some(expected) = self.pending_end() {
                if idx - expected > self.smooth {
                    self.commit(r);
                    self.anchoring = true;
                }
            }
            self.pending.push((idx, samples.to_vec()));
            if self.pending.len() >= ANCHOR_CHUNKS {
                self.commit(r);
            }
            return;
        }
        let head = r.head();
        let diff = idx - head;
        if diff > self.smooth {
            // A real break (the device was quiet): find the new start time.
            self.anchoring = true;
            self.pending.push((idx, samples.to_vec()));
            return;
        }
        if diff < -self.smooth {
            // Far before what we hold (e.g. after silence was filled in while
            // the device was quiet): keep only the part after it.
            let skip = (-diff) as usize;
            if (skip as i64) < frames {
                r.push(&samples[skip * ch..]);
            }
            self.reset();
            return;
        }
        // Continuous. Follow the clocks' slow drift, one frame at a time,
        // judged on the smallest offset of the last ~100 chunks (~1 s).
        self.recent.push_back(diff);
        if self.recent.len() > 100 {
            self.recent.pop_front();
        }
        self.err = *self.recent.iter().min().unwrap() as f64;
        self.since_nudge += 1;
        if self.since_nudge >= NUDGE_EVERY && self.recent.len() >= 50 {
            if self.err > self.drift {
                // Audio sits earlier than its timestamps: stretch by one frame.
                self.since_nudge = 0;
                r.push(&stretch(samples, ch, frames as usize + 1));
                self.shift(-1);
                return;
            } else if self.err < -self.drift && frames > 2 {
                // Audio sits later than its timestamps: squeeze by one frame.
                self.since_nudge = 0;
                r.push(&stretch(samples, ch, frames as usize - 1));
                self.shift(1);
                return;
            }
        }
        r.push(samples);
    }

    /// Where the held chunks end if placed back to back from the first one.
    fn pending_end(&self) -> Option<i64> {
        let (first, _) = self.pending.first()?;
        Some(first + self.pending.iter().map(|(_, s)| (s.len() / self.ch) as i64).sum::<i64>())
    }

    /// Place the held chunks back to back, starting at the earliest time
    /// any of them implies (timestamps are only ever late).
    fn commit(&mut self, r: &mut AudioRing) {
        self.anchoring = false;
        if self.pending.is_empty() {
            return;
        }
        let ch = self.ch;
        let first = self.pending[0].0;
        let mut cum = 0i64;
        let mut start = first;
        for (idx, s) in &self.pending {
            start = start.min(idx - cum);
            cum += (s.len() / ch) as i64;
        }
        let data: Vec<f32> = self.pending.drain(..).flat_map(|(_, s)| s).collect();
        let head = r.head();
        if start >= head {
            r.push_silence((start - head) as usize);
            r.push(&data);
        } else {
            let skip = ((head - start) as usize * ch).min(data.len());
            r.push(&data[skip..]);
        }
        self.reset();
    }

    /// After a break, start judging drift afresh.
    fn reset(&mut self) {
        self.recent.clear();
        self.err = 0.0;
    }

    /// The ring moved by `by` frames against the timestamps: update the offsets.
    fn shift(&mut self, by: i64) {
        for d in self.recent.iter_mut() {
            *d += by;
        }
        self.err += by as f64;
    }

    /// The device has delivered nothing for a while: place anything held,
    /// then fill silence up to `idx`.
    fn idle(&mut self, r: &mut AudioRing, idx: i64) {
        if !self.pending.is_empty() {
            self.commit(r);
            self.anchoring = true; // the next sound starts afresh
        }
        let head = r.head();
        if idx > head {
            r.push_silence((idx - head) as usize);
        }
    }
}

/// Resample interleaved `samples` to `out` frames by linear interpolation,
/// keeping the first and last frame exactly (so chunks still join smoothly).
fn stretch(samples: &[f32], ch: usize, out: usize) -> Vec<f32> {
    let n = samples.len() / ch;
    if n < 2 || out < 2 {
        return samples.to_vec();
    }
    let mut v = Vec::with_capacity(out * ch);
    let scale = (n - 1) as f64 / (out - 1) as f64;
    for i in 0..out {
        let pos = i as f64 * scale;
        let a = (pos.floor() as usize).min(n - 1);
        let b = (a + 1).min(n - 1);
        let t = (pos - a as f64) as f32;
        for c in 0..ch {
            v.push(samples[a * ch + c] * (1.0 - t) + samples[b * ch + c] * t);
        }
    }
    v
}

/// Put captured chunks into the ring (see `Placer`).
fn place(chunks: &Chunks, ring: &Arc<Mutex<AudioRing>>, stop: &AtomicBool) {
    let mut placer = {
        let r = ring.lock().unwrap();
        Placer::new(r.rate, r.channels as usize)
    };
    let mut last_chunk = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(5));
        let batch: Vec<(Instant, Vec<f32>)> = chunks.lock().unwrap().drain(..).collect();
        let mut r = ring.lock().unwrap();
        if !batch.is_empty() {
            last_chunk = Instant::now();
        }
        for (at, samples) in batch {
            let idx = r.frame_at(at);
            placer.place(&mut r, idx, &samples);
        }
        if last_chunk.elapsed() > IDLE_AFTER {
            let idx = r.frame_at(Instant::now().checked_sub(IDLE_LAG).unwrap_or_else(Instant::now));
            placer.idle(&mut r, idx);
        }
    }
}

#[cfg(test)]
mod gain_tests {
    use super::*;

    #[test]
    fn gain_scales_and_clamps() {
        set_mic_gain_db(6.0);
        assert!((mic_gain() - 1.995).abs() < 0.01);
        let mut s = [0.1, -0.2, 0.9];
        apply_gain(&mut s, mic_gain());
        assert!((s[0] - 0.1995).abs() < 0.001 && s[2] == 1.0);
        set_mic_gain_db(0.0);
        assert_eq!(mic_gain(), 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn ring() -> AudioRing {
        AudioRing { rate: RATE, channels: 1, start: Instant::now(), base: 0, data: VecDeque::new(), max_frames: RATE as usize * 600 }
    }

    /// A ramp, so any repeated, skipped or zeroed sample is easy to find.
    fn chunk(from: usize, n: usize) -> Vec<f32> {
        (from..from + n).map(|i| 1.0 + i as f32).collect()
    }

    /// Silent samples after the sound first starts.
    fn zeros(r: &AudioRing) -> usize {
        r.data.iter().skip_while(|x| **x == 0.0).filter(|x| **x == 0.0).count()
    }

    /// Simple deterministic jitter in [-amp, +amp] frames.
    fn jitter(i: usize, amp: i64) -> i64 {
        let x = (i as u64).wrapping_mul(2_654_435_761) % (2 * amp as u64 + 1);
        x as i64 - amp
    }

    #[test]
    fn jittery_timestamps_join_seamlessly() {
        // 10 ms chunks whose timestamps are late by 0, 1 or 2 whole packets
        // (what a Focusrite USB interface really reports) plus a little
        // wobble: the old 3 ms rule turned this into gaps and cut samples.
        let (mut r, mut p) = (ring(), Placer::new(RATE, 1));
        let n = 480;
        for i in 0..3000 {
            let late = [0, 0, 1, 0, 2, 1, 0][i % 7] * n as i64;
            let idx = (i * n) as i64 + late + jitter(i, 24).abs();
            p.place(&mut r, idx, &chunk(i * n, n));
        }
        assert_eq!(zeros(&r), 0, "no silence inserted");
        // Every sample present, in order, no jumps; few or no drift corrections.
        let d: Vec<f32> = r.data.iter().copied().skip_while(|x| *x == 0.0).collect();
        let mut nudges = 0;
        for w in d.windows(2) {
            let step = w[1] - w[0];
            assert!((0.99..=1.01).contains(&step), "jump of {step} between samples");
            if step != 1.0 {
                nudges += 1;
            }
        }
        let nudges = nudges / 479; // a stretched chunk changes ~every step in it
        println!("{nudges} single-frame nudges in 30 s of jittery audio");
        assert!(nudges < 20, "{nudges} nudges for zero real drift");
    }

    #[test]
    fn a_real_break_stays_silent_and_in_place() {
        let (mut r, mut p) = (ring(), Placer::new(RATE, 1));
        p.place(&mut r, 0, &chunk(0, 4800));
        // Device quiet for 500 ms, then sound again at its true time.
        p.place(&mut r, 4800 + 24_000, &chunk(1_000_000, 4800));
        p.idle(&mut r, 0); // place what's held
        assert_eq!(r.head(), 4800 + 24_000 + 4800);
        assert_eq!(zeros(&r), 24_000);
        assert_eq!(r.data[4800 + 24_000], 1.0 + 1_000_000.0);
    }

    #[test]
    fn a_late_first_packet_does_not_shift_the_audio() {
        // The very first packet reports 20 ms late (seen on a Focusrite);
        // the rest are on time or late by one packet. Audio must start at
        // the true time straight away, with no corrections afterwards.
        let (mut r, mut p) = (ring(), Placer::new(RATE, 1));
        let n = 480;
        let base = 48_000; // starts 1 s into the ring
        for i in 0..1000 {
            let late = if i == 0 { 2 * n } else { [0, 1, 0, 0][i % 4] * n } as i64;
            p.place(&mut r, base + (i * n) as i64 + late, &chunk(i * n, n));
        }
        assert_eq!(r.data[base as usize], 1.0, "first sample at its true time");
        assert_eq!(r.head(), base + 1000 * n as i64, "no corrections");
    }

    #[test]
    fn clock_drift_is_followed_gently() {
        // The sound card runs 0.02% (200 ppm, more than real cards) slow
        // against the system clock: over 60 s its timestamps get 12 ms ahead
        // of the samples. Audio must follow without gaps or big jumps.
        let (mut r, mut p) = (ring(), Placer::new(RATE, 1));
        let n = 480;
        let chunks = 6000; // 60 s
        for i in 0..chunks {
            let idx = ((i * n) as f64 * 1.0002).round() as i64;
            p.place(&mut r, idx, &chunk(i * n, n));
        }
        let expected_end = ((chunks * n) as f64 * 1.0002) as i64;
        let off_ms = (r.head() - expected_end).abs() as f64 / RATE as f64 * 1000.0;
        // Corrections start past DRIFT_MS, so it settles just beyond that.
        assert!(off_ms < 8.0, "audio {off_ms:.1} ms away from its timestamps");
        assert_eq!(zeros(&r), 0);
        let d: Vec<f32> = r.data.iter().copied().collect();
        // f32 holds values near 3e6 to 1/4, so allow that; a repeated or
        // skipped sample would show as a step of 0 or 2.
        if let Some((k, w)) = d.windows(2).enumerate().find(|(_, w)| !(0.5..=1.5).contains(&(w[1] - w[0]))) {
            panic!("jump at frame {k}: {} -> {}", w[0], w[1]);
        }
    }

    #[test]
    fn idle_fills_silence_then_sound_lands_at_its_time() {
        let (mut r, mut p) = (ring(), Placer::new(RATE, 1));
        p.place(&mut r, 0, &chunk(0, 480));
        p.idle(&mut r, 48_000); // quiet: filled up to 1 s
        assert_eq!(r.head(), 48_000);
        // Sound resumes at 1.2 s.
        p.place(&mut r, 57_600, &chunk(5_000_000, 480));
        p.idle(&mut r, 0);
        assert_eq!(r.head(), 57_600 + 480);
        assert_eq!(r.data[57_600], 1.0 + 5_000_000.0);
    }

    /// The old rule (before 0.1.10): join only within 3 ms, else insert
    /// silence or cut the overlap. Counts how often it would have done so.
    fn old_rule_glitches(timeline: &[(i64, usize)], rate: u32) -> (usize, usize) {
        let tol = rate as i64 * 3 / 1000;
        let (mut head, mut gaps, mut cuts) = (timeline[0].0, 0, 0);
        for &(idx, frames) in timeline {
            let d = idx - head;
            if d.abs() <= tol {
                head += frames as i64;
            } else if d > 0 {
                gaps += 1;
                head = idx + frames as i64;
            } else {
                cuts += 1;
                head = head.max(idx + frames as i64);
            }
        }
        (gaps, cuts)
    }

    /// Records 15 s of real timestamps from the mic and desktop audio and
    /// replays them through the old rule and the new Placer.
    /// cargo test live_device_timestamps -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_device_timestamps() {
        let (mic, _) = default_devices();
        println!("default mic: {mic:?}");
        for (label, src) in [
            ("mic", Source::Mic { device: String::new(), mono: true }),
            ("desktop", Source::Desktop),
        ] {
            let chunks: Chunks = Arc::new(Mutex::new(VecDeque::new()));
            let (stop, _q, rate, ch, name) = open(src, Some(chunks.clone())).expect("device");
            let t0 = Instant::now();
            let mut got: Vec<(Instant, Vec<f32>)> = Vec::new();
            while t0.elapsed() < Duration::from_secs(15) {
                thread::sleep(Duration::from_millis(5));
                got.extend(chunks.lock().unwrap().drain(..));
            }
            stop.store(true, Ordering::Relaxed);
            if got.len() < 10 {
                println!("{label} ({name}): only {} chunks (silent loopback?)", got.len());
                continue;
            }
            let ch = ch as usize;
            let start = got[0].0;
            let at = |t: Instant| ((t - start).as_secs_f64() * rate as f64).round() as i64;
            let timeline: Vec<(i64, usize)> = got.iter().map(|(t, s)| (at(*t), s.len() / ch)).collect();
            // Jitter: timestamp position minus position if samples were continuous.
            let mut cum = 0i64;
            let mut jit: Vec<f64> = Vec::new();
            for &(idx, frames) in &timeline {
                jit.push((idx - cum) as f64 * 1000.0 / rate as f64);
                cum += frames as i64;
            }
            let mean = jit.iter().sum::<f64>() / jit.len() as f64;
            let dev: Vec<f64> = jit.iter().map(|j| (j - mean).abs()).collect();
            let worst = dev.iter().cloned().fold(0.0, f64::max);
            let over3 = dev.iter().filter(|d| **d > 3.0).count();
            let (gaps, cuts) = old_rule_glitches(&timeline, rate);
            // Clock drift: slope of timestamp position against samples delivered.
            let xs: Vec<f64> = { let mut c = 0f64; timeline.iter().map(|&(_, f)| { let x = c; c += f as f64; x }).collect() };
            let ys: Vec<f64> = timeline.iter().map(|&(i, _)| i as f64).collect();
            let (mx, my) = (xs.iter().sum::<f64>() / xs.len() as f64, ys.iter().sum::<f64>() / ys.len() as f64);
            let slope = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum::<f64>() / xs.iter().map(|x| (x - mx).powi(2)).sum::<f64>();
            println!("   clock drift vs system clock: {:+.0} ppm", (slope - 1.0) * 1e6);
            // New: replay through the Placer.
            let mut r = AudioRing { rate, channels: ch as u16, start, base: 0, data: VecDeque::new(), max_frames: rate as usize * 60 };
            let mut p = Placer::new(rate, ch);
            let total: usize = got.iter().map(|(_, s)| s.len() / ch).sum();
            let half = got.len() / 2;
            let mut first_half = 0i64;
            for (k, (t, s)) in got.iter().enumerate() {
                let before = r.head();
                p.place(&mut r, at(*t), s);
                if std::env::var("REWIND_TRACE").is_ok() && k % 50 == 0 {
                    println!("   k={k} diff={:+.2}ms err={:+.2}ms frames={} placed={}", (at(*t) - before) as f64 * 1000.0 / rate as f64, p.err * 1000.0 / rate as f64, s.len() / ch, r.head() - before);
                }
                if k + 1 == half {
                    first_half = r.head() - got[..half].iter().map(|(_, s)| (s.len() / ch) as i64).sum::<i64>();
                }
            }
            let held = (r.head() - r.base as i64) as usize;
            println!(
                "{label} ({name}, {rate} Hz): {} chunks of ~{} frames; timestamp wobble avg {:.2} ms, worst {:.2} ms, {} chunks >3 ms off",
                got.len(), total / got.len(), dev.iter().sum::<f64>() / dev.len() as f64, worst, over3
            );
            println!("   old rule: {gaps} silence gaps + {cuts} cut overlaps in 15 s");
            let all = held as i64 - total as i64;
            println!("   new: {total} frames captured, {held} placed, 0 gaps; corrections {first_half:+} frames in the first 7.5 s, {:+} after", all - first_half);
        }
    }
}

