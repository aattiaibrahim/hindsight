//! The native capture pipeline: WGC → GPU convert → NVENC → RAM ring, with
//! audio in RAM rings on the same clock. Saving cuts video and audio for the
//! exact same time window, so they line up by construction.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::capture::Capture;
use super::gpu::{self, Converter, Gpu};
use super::nvenc::{Codec, Encoder, EncoderConfig};
use crate::audio::{self, AudioRing};
use crate::{ffmpeg, monitors, settings::Settings};

pub struct VPacket {
    pub index: u64,
    pub key: bool,
    pub data: Arc<Vec<u8>>,
}

pub struct VideoRing {
    pub codec: Codec,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Time of frame 0 on the shared clock.
    pub start: Instant,
    pub packets: VecDeque<VPacket>,
    max_frames: usize,
}

impl VideoRing {
    pub fn time(&self, index: u64) -> Instant {
        self.start + Duration::from_secs_f64(index as f64 / self.fps as f64)
    }
    fn push(&mut self, p: VPacket) {
        self.packets.push_back(p);
        // Keep the buffer length, then make sure the oldest packet is a keyframe.
        while self.packets.len() > self.max_frames {
            self.packets.pop_front();
        }
        while self.packets.front().is_some_and(|p| !p.key) {
            self.packets.pop_front();
        }
    }
    fn index_at_or_after(&self, t: Instant) -> Option<usize> {
        self.packets.iter().position(|p| self.time(p.index) >= t)
    }
}

#[derive(Clone)]
pub struct AudioTrack {
    pub title: &'static str,
    pub bitrate: &'static str,
    pub ring: Arc<Mutex<AudioRing>>,
}

/// What one capture run holds. Kept after the run stops, so clips can reach
/// back across a restart (Win+P, device change) when the format matches.
pub struct Buffers {
    pub key: String,
    pub video: Mutex<VideoRing>,
    pub audio: Vec<AudioTrack>,
}

#[derive(Serialize, Deserialize, Clone)]
struct PartMeta {
    format: String,
    fps: u32,
    tracks: Vec<TrackMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
struct TrackMeta {
    title: String,
    bitrate: String,
    rate: u32,
    channels: u16,
}

/// A recording in progress: video and audio stream straight to files.
struct RecSink {
    dir: PathBuf,
    part: u32,
    first: Option<u64>,
    video: File,
    audio: Vec<File>,
    audio_written: Vec<u64>,
}

pub struct Pipeline {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub buffers: Arc<Buffers>,
    _audio: Vec<audio::Capture>,
    pub last_frame: Arc<Mutex<Instant>>,
    pub error: Arc<Mutex<Option<String>>>,
    pub labels: Vec<String>,
    pub audio_error: Option<String>,
    pub adapter: String,
    rec: Arc<Mutex<Option<RecSink>>>,
}

impl Pipeline {
    pub fn start(s: &Settings, m: &monitors::Monitor, out_w: u32, out_h: u32, key: String) -> Result<Pipeline, String> {
        let monitor_name = m.name.clone();
        let codec = Codec::from_setting(&s.encoder);
        let fps = s.fps;
        let keep_secs = s.buffer_seconds + 5;

        // Audio first: its clock starts now, like the video's.
        let mut caps = Vec::new();
        let mut tracks = Vec::new();
        let mut labels = Vec::new();
        let mut errors = Vec::new();
        let desktop = if s.desktop_audio && s.discord_track { audio::Source::DesktopNoDiscord } else { audio::Source::Desktop };
        if s.desktop_audio {
            match audio::start_ring(desktop, keep_secs) {
                Ok((c, ring)) => {
                    labels.push(format!("Desktop: {}", c.device));
                    tracks.push(AudioTrack { title: "Desktop audio", bitrate: "192k", ring });
                    caps.push(c);
                }
                Err(e) => errors.push(format!("Desktop audio: {e}")),
            }
        }
        if s.mic_enabled {
            match audio::start_ring(audio::Source::Mic { device: s.mic_device.clone(), mono: s.mic_mono }, keep_secs) {
                Ok((c, ring)) => {
                    labels.push(format!("Mic{}: {}", if c.channels == 1 { " (mono)" } else { "" }, c.device));
                    tracks.push(AudioTrack { title: "Microphone", bitrate: "128k", ring });
                    caps.push(c);
                }
                Err(e) => errors.push(format!("Mic: {e}")),
            }
        }
        if s.desktop_audio && s.discord_track {
            match audio::start_ring(audio::Source::Discord, keep_secs) {
                Ok((c, ring)) => {
                    labels.push("Discord (own track)".into());
                    tracks.push(AudioTrack { title: "Discord audio", bitrate: "160k", ring });
                    caps.push(c);
                }
                Err(e) => errors.push(format!("Discord: {e}")),
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let last_frame = Arc::new(Mutex::new(Instant::now()));
        let error = Arc::new(Mutex::new(None));
        let rec: Arc<Mutex<Option<RecSink>>> = Arc::new(Mutex::new(None));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Arc<Buffers>, String>>();

        let quality = s.quality;
        let cursor = s.draw_cursor;
        let capture_area = s.capture_area;
        let tracks_for_thread = tracks.clone();
        let (stop2, last2, err2, rec2) = (stop.clone(), last_frame.clone(), error.clone(), rec.clone());
        let thread = thread::Builder::new()
            .name("rewind-capture".into())
            .spawn(move || {
                let setup = (|| -> Result<(Gpu, Capture, Converter, Encoder), String> {
                    // Look up the monitor on this thread (its handles can't cross threads).
                    let (adapter, hmon, adapter_name) =
                        gpu::find_output(&monitor_name).ok_or("Monitor not found on any GPU")?;
                    let g = Gpu::new(&adapter, adapter_name).map_err(|e| format!("GPU: {e}"))?;
                    let cap = Capture::start(&g, hmon, cursor).map_err(|e| format!("Screen capture: {e}"))?;
                    let (w, h) = {
                        let l = cap.latest.lock().unwrap();
                        (l.width, l.height)
                    };
                    // The recording area, measured on the captured frame itself.
                    let area = capture_area.map(|a| {
                        let (x, y, aw, ah) = a.pixels(w as i32, h as i32);
                        (x as u32, y as u32, aw as u32, ah as u32)
                    });
                    let conv = Converter::new(&g, w, h, out_w, out_h, fps, area).map_err(|e| format!("GPU scaler: {e}"))?;
                    let enc = Encoder::new(&g, EncoderConfig { codec, width: out_w, height: out_h, fps, quality, gop: fps })?;
                    Ok((g, cap, conv, enc))
                })();
                let (_g, cap, conv, mut enc) = match setup {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let t0 = Instant::now();
                let buffers = Arc::new(Buffers {
                    key,
                    video: Mutex::new(VideoRing {
                        codec,
                        fps,
                        width: out_w,
                        height: out_h,
                        start: t0,
                        packets: VecDeque::new(),
                        max_frames: (keep_secs * fps) as usize,
                    }),
                    audio: tracks_for_thread.clone(),
                });
                let _ = ready_tx.send(Ok(buffers.clone()));
                let frame = Duration::from_secs_f64(1.0 / fps as f64);
                let mut n: u64 = 0;
                while !stop2.load(Ordering::Relaxed) {
                    let due = t0 + frame * n as u32;
                    let now = Instant::now();
                    if due > now {
                        spin_sleep(due - now);
                    }
                    if cap.latest.lock().unwrap().size_changed {
                        *err2.lock().unwrap() = Some("The screen resolution changed.".into());
                        return;
                    }
                    let src = cap.latest.lock().unwrap().texture.clone();
                    let packet = conv
                        .convert(&src, enc.next_input())
                        .map_err(|e| format!("GPU scaler: {e}"))
                        .and_then(|_| enc.encode(n == 0));
                    let p = match packet {
                        Ok(p) => p,
                        Err(e) => {
                            *err2.lock().unwrap() = Some(e);
                            return;
                        }
                    };
                    let vp = VPacket { index: p.index, key: p.key, data: Arc::new(p.data) };
                    if let Some(r) = rec2.lock().unwrap().as_mut() {
                        record_frame(r, &vp, &buffers);
                    }
                    buffers.video.lock().unwrap().push(vp);
                    *last2.lock().unwrap() = Instant::now();
                    n += 1;
                }
            })
            .map_err(|e| e.to_string())?;

        let buffers = ready_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "The capture didn't start in time.".to_string())??;
        Ok(Pipeline {
            stop,
            thread: Some(thread),
            buffers,
            _audio: caps,
            last_frame,
            error,
            labels,
            audio_error: if errors.is_empty() { None } else { Some(errors.join(" · ")) },
            adapter: String::new(),
            rec,
        })
    }

    pub fn failed(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn frame_age(&self) -> Duration {
        self.last_frame.lock().unwrap().elapsed()
    }

    /// Start writing this run into `dir` as recording part `part`.
    pub fn begin_recording(&self, dir: &Path, part: u32) -> Result<(), String> {
        let v = self.buffers.video.lock().unwrap();
        let meta = PartMeta {
            format: v.codec.raw_format().into(),
            fps: v.fps,
            tracks: self
                .buffers
                .audio
                .iter()
                .map(|t| {
                    let r = t.ring.lock().unwrap();
                    TrackMeta { title: t.title.into(), bitrate: t.bitrate.into(), rate: r.rate, channels: r.channels }
                })
                .collect(),
        };
        drop(v);
        let stem = part_stem(part, &self.buffers.key);
        fs::write(dir.join(format!("{stem}.json")), serde_json::to_vec(&meta).unwrap()).map_err(|e| e.to_string())?;
        let video = File::create(dir.join(format!("{stem}.video"))).map_err(|e| e.to_string())?;
        let mut audio = Vec::new();
        for i in 0..meta.tracks.len() {
            audio.push(File::create(dir.join(format!("{stem}.a{i}"))).map_err(|e| e.to_string())?);
        }
        let n = meta.tracks.len();
        let mut sink = RecSink { dir: dir.to_path_buf(), part, first: None, video, audio, audio_written: vec![0; n] };
        // Lock order matches the capture thread (recording, then video), so the
        // next frame it encodes follows straight on from what we write here.
        let mut guard = self.rec.lock().unwrap();
        {
            // Start from the most recent keyframe, so nothing after the click
            // is lost (the recording may begin up to a second early instead).
            let v = self.buffers.video.lock().unwrap();
            if let Some(pos) = v.packets.iter().rposition(|p| p.key) {
                for p in v.packets.range(pos..) {
                    sink.first.get_or_insert(p.index);
                    sink.video.write_all(&p.data).map_err(|e| e.to_string())?;
                }
            }
        }
        flush_audio(&mut sink, &self.buffers, false);
        *guard = Some(sink);
        Ok(())
    }

    /// Stop writing; waits briefly so the audio catches up with the last frame.
    pub fn end_recording(&self) -> Option<(PathBuf, u32)> {
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline && !audio_caught_up(&self.buffers) {
            thread::sleep(Duration::from_millis(20));
        }
        let mut sink = self.rec.lock().unwrap().take()?;
        flush_audio(&mut sink, &self.buffers, true);
        Some((sink.dir, sink.part))
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.end_recording();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Rust's sleep on Windows uses a high-resolution waitable timer (sub-ms),
/// so no busy-waiting is needed. Frame timestamps come from the frame index,
/// so a slightly late wake-up never shifts the video timeline.
fn spin_sleep(d: Duration) {
    thread::sleep(d);
}

fn part_stem(part: u32, key: &str) -> String {
    format!("{part:06}_{key}")
}

fn record_frame(r: &mut RecSink, p: &VPacket, b: &Buffers) {
    if r.first.is_none() {
        if !p.key {
            return; // a recording starts on a keyframe
        }
        r.first = Some(p.index);
    }
    let _ = r.video.write_all(&p.data);
    flush_audio(r, b, false);
}

/// Append the audio that belongs with the frames written so far.
fn flush_audio(r: &mut RecSink, b: &Buffers, final_flush: bool) {
    let Some(first) = r.first else { return };
    let v = b.video.lock().unwrap();
    let start = v.time(first);
    let last = v.packets.back().map(|p| p.index).unwrap_or(first);
    let video_end = v.time(last + 1);
    drop(v);
    for (i, t) in b.audio.iter().enumerate() {
        let ring = t.ring.lock().unwrap();
        // Audio lags real time a little; only take what has actually arrived.
        let until = if final_flush { video_end } else { ring.end_time().min(video_end) };
        let want = if until > start { ((until - start).as_secs_f64() * ring.rate as f64).round() as u64 } else { 0 };
        if want > r.audio_written[i] {
            let from = start + Duration::from_secs_f64(r.audio_written[i] as f64 / ring.rate as f64);
            let pcm = ring.slice(from, (want - r.audio_written[i]) as usize);
            drop(ring);
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            let _ = r.audio[i].write_all(&bytes);
            r.audio_written[i] = want;
        }
    }
}

fn audio_caught_up(b: &Buffers) -> bool {
    let v = b.video.lock().unwrap();
    let Some(last) = v.packets.back() else { return true };
    let end = v.time(last.index + 1);
    drop(v);
    b.audio.iter().all(|t| t.ring.lock().unwrap().end_time() >= end)
}

/// One stretch of one run to put in a clip: packets [from, to] (indices).
struct Span {
    buffers: Arc<Buffers>,
    from: u64,
    to: u64,
}

/// Save the last `seconds` across `runs` (oldest first) into `out`.
pub fn save(runs: &[Arc<Buffers>], seconds: u32, out: &Path, scratch: &Path) -> Result<(), String> {
    let newest = runs.last().ok_or("The replay buffer isn't running yet.")?;
    // Make sure the audio for the newest frames has arrived (it trails ~0.1 s).
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline && !audio_caught_up(newest) {
        thread::sleep(Duration::from_millis(20));
    }
    let end = {
        let v = newest.video.lock().unwrap();
        let last = v.packets.back().ok_or("Nothing buffered yet. Wait a moment.")?;
        v.time(last.index + 1)
    };
    let want_from = end - Duration::from_secs(seconds as u64);

    let mut spans: Vec<Span> = Vec::new();
    for run in runs.iter().rev() {
        if run.key != newest.key {
            break;
        }
        let v = run.video.lock().unwrap();
        let (Some(first), Some(last)) = (v.packets.front(), v.packets.back()) else { continue };
        let (first_i, last_i) = (first.index, last.index);
        // Start on the last keyframe at or before the wanted start.
        let from = match v.index_at_or_after(want_from) {
            Some(pos) => {
                let mut pos = pos;
                while pos > 0 && !v.packets[pos].key {
                    pos -= 1;
                }
                v.packets[pos].index
            }
            None => continue, // this run ended before the window
        };
        drop(v);
        spans.insert(0, Span { buffers: run.clone(), from, to: last_i });
        if from > first_i || run.video.lock().unwrap().time(first_i) <= want_from {
            break; // the window starts inside this run
        }
    }
    if spans.is_empty() {
        return Err("Nothing buffered yet. Wait a moment.".into());
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Can't create clip folder: {e}"))?;
    }
    let stamp = chrono::Local::now().format("%H%M%S%f");
    let staging = out.parent().unwrap().join(format!(".rewind-save-{stamp}"));
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| {
        if spans.len() == 1 {
            return mux_span(&spans[0], out, &staging, true);
        }
        let mut parts = Vec::new();
        for (i, s) in spans.iter().enumerate() {
            let p = staging.join(format!("part{i}.mkv"));
            mux_span(s, &p, &staging, false)?;
            parts.push(p);
        }
        crate::engine::concat_files(&parts, out, scratch)
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

/// What a save's staging folder needs to be finished later, if Hindsight is
/// closed (or crashes) between writing the raw files and the MKV.
#[derive(Serialize, Deserialize)]
struct SaveMeta {
    out: PathBuf,
    format: String,
    fps: u32,
    tracks: Vec<TrackMeta>,
}

const SAVE_DIR_PREFIX: &str = ".rewind-save-";

fn mux_span(s: &Span, out: &Path, staging: &Path, resumable: bool) -> Result<(), String> {
    let v = s.buffers.video.lock().unwrap();
    let start = v.time(s.from);
    let frames = s.to - s.from + 1;
    let video_path = staging.join("video.raw");
    let mut vf = File::create(&video_path).map_err(|e| e.to_string())?;
    for p in v.packets.iter().filter(|p| p.index >= s.from && p.index <= s.to) {
        vf.write_all(&p.data).map_err(|e| e.to_string())?;
    }
    drop(vf);
    let (fps, format) = (v.fps, v.codec.raw_format());
    drop(v);
    let secs = frames as f64 / fps as f64;
    let mut tracks = Vec::new();
    for (i, t) in s.buffers.audio.iter().enumerate() {
        let ring = t.ring.lock().unwrap();
        let pcm = ring.slice(start, (secs * ring.rate as f64).round() as usize);
        let path = staging.join(format!("a{i}.raw"));
        let bytes: Vec<u8> = pcm.iter().flat_map(|x| x.to_le_bytes()).collect();
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        tracks.push((path, TrackMeta { title: t.title.into(), bitrate: t.bitrate.into(), rate: ring.rate, channels: ring.channels }));
    }
    if resumable {
        let meta = SaveMeta { out: out.to_path_buf(), format: format.to_string(), fps, tracks: tracks.iter().map(|(_, t)| t.clone()).collect() };
        let _ = fs::write(staging.join("save.json"), serde_json::to_vec(&meta).unwrap());
    }
    mux(&video_path, format, fps, &tracks, out, Duration::from_secs_f64(60.0 + secs))
}

/// Finish saves that were cut off (Hindsight closed or crashed mid-save).
/// Returns the clips written.
pub fn recover_saves(clips_dir: &Path) -> Vec<String> {
    let mut done = Vec::new();
    let tops = std::iter::once(clips_dir.to_path_buf())
        .chain(fs::read_dir(clips_dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    for top in tops {
        for e in fs::read_dir(&top).into_iter().flatten().flatten() {
            let dir = e.path();
            if !dir.is_dir() || !e.file_name().to_string_lossy().starts_with(SAVE_DIR_PREFIX) {
                continue;
            }
            let Some(meta) = fs::read(dir.join("save.json")).ok().and_then(|b| serde_json::from_slice::<SaveMeta>(&b).ok()) else {
                continue; // from an older version, or not finished writing: leave it alone
            };
            let tracks: Vec<(PathBuf, TrackMeta)> =
                meta.tracks.into_iter().enumerate().map(|(i, t)| (dir.join(format!("a{i}.raw")), t)).collect();
            let video = dir.join("video.raw");
            // Rough length from the size (a few MB per second at most), just to size the time limit.
            let est_secs = fs::metadata(&video).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(60.0);
            // Write next to the clip first, so a half-written older attempt is only replaced by a good one.
            let tmp = meta.out.with_extension("recovering.mkv");
            match mux(&video, &meta.format, meta.fps, &tracks, &tmp, Duration::from_secs_f64(120.0 + est_secs))
                .and_then(|_| fs::rename(&tmp, &meta.out).map_err(|e| e.to_string()))
            {
                Ok(()) => {
                    let _ = fs::remove_dir_all(&dir);
                    done.push(meta.out.to_string_lossy().into_owned());
                }
                Err(e) => {
                    crate::log!("[rewind] couldn't finish the cut-off save in {}: {e}", dir.display());
                    let _ = fs::remove_file(&tmp);
                }
            }
        }
    }
    done
}

/// Raw NVENC stream + raw PCM tracks → MKV (video copied, audio to AAC).
/// If ffmpeg fails or takes longer than `limit`, try once more with
/// lossless FLAC audio, so a clip is never lost to an audio-encoder problem.
fn mux(video: &Path, format: &str, fps: u32, tracks: &[(PathBuf, TrackMeta)], out: &Path, limit: Duration) -> Result<(), String> {
    match mux_with(video, format, fps, tracks, out, limit, "aac") {
        Ok(()) => Ok(()),
        Err(e) if !tracks.is_empty() => {
            crate::log!("[rewind] clip audio as AAC failed ({e}); saving with FLAC audio instead");
            mux_with(video, format, fps, tracks, out, limit, "flac")
        }
        Err(e) => Err(e),
    }
}

fn mux_with(video: &Path, format: &str, fps: u32, tracks: &[(PathBuf, TrackMeta)], out: &Path, limit: Duration, acodec: &str) -> Result<(), String> {
    let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-r", &fps.to_string(), "-f", format, "-i"]);
    a.push(video.into());
    for (p, t) in tracks {
        a.extend(ffmpeg::args(&["-f", "f32le", "-ar", &t.rate.to_string(), "-ac", &t.channels.to_string(), "-i"]));
        a.push(p.into());
    }
    a.extend(ffmpeg::args(&["-map", "0:v"]));
    // With more than one track (desktop, mic, Discord), the first one is all of them
    // mixed: other players (VLC, browsers, phones, the NAS app) only play the first
    // track, and a clip whose desktop track was quiet seemed to have no sound at all.
    // Hindsight itself leaves this track out and uses the separate ones
    // (library::mix_copy finds it by its name), so nothing plays twice.
    let mixed = tracks.len() >= 2;
    if mixed {
        let ins: String = (1..=tracks.len()).map(|i| format!("[{i}:a]aresample=48000,aformat=channel_layouts=stereo[m{i}];")).collect();
        let refs: String = (1..=tracks.len()).map(|i| format!("[m{i}]")).collect();
        a.extend(ffmpeg::args(&["-filter_complex", &format!("{ins}{refs}amix=inputs={}:duration=longest:normalize=0[mix]", tracks.len())]));
        a.extend(ffmpeg::args(&["-map", "[mix]", "-b:a:0", "192k", "-metadata:s:a:0", &format!("title={}", crate::library::MIX_TITLE)]));
    }
    let first = usize::from(mixed);
    for (i, (_, t)) in tracks.iter().enumerate() {
        let o = i + first;
        a.extend(ffmpeg::args(&["-map", &format!("{}:a", i + 1)]));
        a.push(format!("-b:a:{o}").into());
        a.push(t.bitrate.clone().into());
        a.push(format!("-metadata:s:a:{o}").into());
        a.push(format!("title={}", t.title).into());
    }
    a.extend(ffmpeg::args(&["-c:v", "copy"]));
    if !tracks.is_empty() {
        a.extend(ffmpeg::args(&["-c:a", acodec, "-ar", "48000"]));
    }
    a.push(out.into());
    ffmpeg::run_limited("ffmpeg", &a, Some(limit)).map(|_| ())
}

/// Turn raw recording parts in `dir` into `NNNNNN_key.mkv` files, which the
/// existing recording finalizer joins (also used for crash recovery).
pub fn mux_recording_parts(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut metas: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    metas.sort();
    for meta_path in metas {
        let Ok(meta) = fs::read(&meta_path).map_err(|_| ()).and_then(|b| serde_json::from_slice::<PartMeta>(&b).map_err(|_| ())) else {
            continue;
        };
        let stem = meta_path.file_stem().unwrap().to_string_lossy().to_string();
        let video = dir.join(format!("{stem}.video"));
        if fs::metadata(&video).map(|m| m.len()).unwrap_or(0) == 0 {
            continue;
        }
        let tracks: Vec<(PathBuf, TrackMeta)> =
            meta.tracks.iter().enumerate().map(|(i, t)| (dir.join(format!("{stem}.a{i}")), t.clone())).collect();
        let out = dir.join(format!("{stem}.mkv"));
        // Generous: about 1 s per MB of video, so only a stuck ffmpeg hits it.
        let limit = Duration::from_secs(300 + fs::metadata(&video).map(|m| m.len() / 1_000_000).unwrap_or(0));
        if mux(&video, &meta.format, meta.fps, &tracks, &out, limit).is_ok() {
            let _ = fs::remove_file(&meta_path);
            let _ = fs::remove_file(&video);
            for (p, _) in &tracks {
                let _ = fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// Max loudness (dB) of one audio track of `path`, between `from` and `to` seconds.
    fn loudness(path: &Path, track: usize, from: f64, to: f64) -> f64 {
        let mut a = ffmpeg::args(&["-hide_banner", "-nostats", "-ss", &format!("{from}"), "-to", &format!("{to}"), "-i"]);
        a.push(path.into());
        a.extend(ffmpeg::args(&["-map", &format!("0:a:{track}"), "-af", "volumedetect", "-f", "null", "-"]));
        let out = ffmpeg::cmd("ffmpeg").args(&a).output().unwrap();
        let log = String::from_utf8_lossy(&out.stderr);
        log.lines().find_map(|l| l.split("max_volume:").nth(1)).and_then(|s| s.trim().trim_end_matches(" dB").parse().ok()).unwrap_or(-999.0)
    }

    /// Like the desktop recording that seemed silent: nothing on the desktop track,
    /// sound on the mic and Discord. The saved clip's first track is all of them
    /// mixed (what other players play), Hindsight's own mixes leave it out, and an
    /// edit that mutes the mic reaches the mixed track too.
    #[test]
    fn saved_clips_start_with_all_their_sound_mixed() {
        ffmpeg::init(None);
        let d = std::env::temp_dir().join(format!("rewind-mixtrack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=60", "-t", "4", "-c:v", "h264_nvenc", "-g", "60", "-bf", "0", "-f", "h264"]);
        a.push(d.join("video.raw").into());
        ffmpeg::run("ffmpeg", &a).unwrap();
        let raw = |name: &str, src: &str, ch: &str| {
            let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", src, "-t", "4", "-ar", "48000", "-ac", ch, "-f", "f32le"]);
            a.push(d.join(name).into());
            ffmpeg::run("ffmpeg", &a).unwrap();
        };
        raw("desktop.raw", "anullsrc=r=48000:cl=stereo", "2");
        raw("mic.raw", "sine=frequency=440:sample_rate=48000", "1");
        raw("discord.raw", "sine=frequency=880:sample_rate=48000", "2");
        let t = |title: &str, ch: u16| TrackMeta { title: title.into(), bitrate: "128k".into(), rate: 48000, channels: ch };
        let tracks = vec![(d.join("desktop.raw"), t("Desktop audio", 2)), (d.join("mic.raw"), t("Microphone", 1)), (d.join("discord.raw"), t("Discord audio", 2))];
        let clip = d.join("Desktop clip.mkv");
        mux(&d.join("video.raw"), "h264", 60, &tracks, &clip, Duration::from_secs(60)).unwrap();
        let c = clip.to_string_lossy().into_owned();

        let titles = crate::library::track_titles(&c);
        assert_eq!(titles, vec!["All audio", "Desktop audio", "Microphone", "Discord audio"]);
        assert!(loudness(&clip, 0, 0.0, 4.0) > -20.0, "other players hear the mic and Discord");
        assert!(loudness(&clip, 1, 0.0, 4.0) < -60.0, "the desktop track is still silent on its own");
        assert_eq!(crate::library::mix_copy(&titles), Some(0));
        assert_eq!(crate::library::tracks_to_mix(&c, 4), vec![1, 2, 3], "Hindsight's own mixes leave the All audio track out");

        // Mute the mic and Discord from 1 s to 3 s, save the trim: the All audio track is quiet there too.
        let edit: crate::audio_edit::AudioEdit = serde_json::from_value(serde_json::json!([
            { "gainDb": 0.0, "mutes": [] },
            { "gainDb": 0.0, "mutes": [] },
            { "gainDb": 0.0, "mutes": [[1.0, 3.0]] },
            { "gainDb": 0.0, "mutes": [[1.0, 3.0]] }
        ]))
        .unwrap();
        let out = crate::library::trim(&c, 0.0, 4.0, true, &edit).unwrap();
        crate::library::refresh_mix_track(&out).unwrap();
        let o = Path::new(&out);
        assert_eq!(crate::library::track_titles(&out), vec!["All audio", "Desktop audio", "Microphone", "Discord audio"]);
        assert!(loudness(o, 0, 1.4, 2.6) < -60.0, "the muted stretch is muted in All audio too");
        assert!(loudness(o, 0, 3.3, 3.9) > -20.0, "and the rest still has sound");
        let _ = fs::remove_dir_all(&d);
    }
    /// A save cut off after its raw files were written (Hindsight closed or
    /// crashed) is finished on the next start, and its folder removed.
    #[test]
    fn unfinished_save_is_finished_on_next_start() {
        ffmpeg::init(None);
        let clips = std::env::temp_dir().join(format!("rewind-recover-save-{}", std::process::id()));
        let game = clips.join("Some Game");
        let staging = game.join(format!("{SAVE_DIR_PREFIX}123"));
        fs::create_dir_all(&staging).unwrap();
        let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=60", "-t", "2", "-c:v", "h264_nvenc", "-bf", "0", "-f", "h264"]);
        a.push(staging.join("video.raw").into());
        ffmpeg::run("ffmpeg", &a).expect("test video");
        let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:r=44100:d=2", "-ac", "1", "-f", "f32le"]);
        a.push(staging.join("a0.raw").into());
        ffmpeg::run("ffmpeg", &a).expect("test audio");
        let out = game.join("Some Game clip.mkv");
        let meta = SaveMeta {
            out: out.clone(),
            format: "h264".into(),
            fps: 60,
            tracks: vec![TrackMeta { title: "Microphone".into(), bitrate: "128k".into(), rate: 44100, channels: 1 }],
        };
        fs::write(staging.join("save.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        // A folder without save.json (older version) is left alone.
        let old = game.join(format!("{SAVE_DIR_PREFIX}old"));
        fs::create_dir_all(&old).unwrap();

        let done = recover_saves(&clips);
        assert_eq!(done, vec![out.to_string_lossy().into_owned()]);
        let p = crate::library::probe(&out.to_string_lossy()).unwrap();
        assert_eq!((p.width, p.audio_tracks), (320, 1));
        assert!((p.duration - 2.0).abs() < 0.2, "{}", p.duration);
        assert!(!staging.exists());
        assert!(old.exists());
        let _ = fs::remove_dir_all(&clips);
    }
}
