//! The replay buffer and recorder.
//!
//! ffmpeg grabs the monitor on the GPU (ddagrab) and encodes with NVENC into
//! 2-second MKV segments in a rolling ring on disk. Saving a clip joins the
//! last N segments without re-encoding.
//!
//! Recording reuses the same capture: while it runs, every finished segment is
//! copied out of the ring into a hidden folder next to the final file, and on
//! stop they are joined. If Hindsight crashes mid-recording, the copied segments
//! are still there and get recovered on the next start.
//!
//! A supervisor thread checks the monitor layout every second. When it changes
//! (Win+P, a monitor plugged in, a resolution change) or ffmpeg dies, it starts
//! a fresh "generation" on the right monitor, so capture never stays black.

use crate::{audio, ffmpeg, games, monitors, native, settings::Settings};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::io::{ErrorKind, Read};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const SEG_SECONDS: u32 = 2;
/// Video starts ~0.12 s before ffmpeg connects to the audio sockets (measured
/// with a flash + click test), so audio is shifted later by this much.
const AUDIO_DELAY_SECS: f64 = 0.12;
/// Refuse to save a clip with less free space than this.
const MIN_FREE_FOR_CLIP: u64 = 300 * 1024 * 1024;
/// Stop a recording automatically below this much free space.
const MIN_FREE_FOR_RECORDING: u64 = 1536 * 1024 * 1024;
pub const RECORDING_DIR_PREFIX: &str = ".rewind-recording-";

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub running: bool,
    pub monitor: Option<String>,
    pub width: i32,
    pub height: i32,
    pub encoder: String,
    pub fps: u32,
    pub buffered_seconds: u32,
    pub buffer_seconds: u32,
    pub restarts: u32,
    pub last_error: Option<String>,
    /// e.g. ["Desktop: Speakers (Realtek)", "Mic (mono): USB Microphone"]
    pub audio: Vec<String>,
    pub audio_error: Option<String>,
    /// The game in front, if one is detected
    pub game: Option<String>,
    pub recording: bool,
    pub recording_seconds: u32,
    /// Set when a recording finishes (incl. auto-stop); the app turns it into an event
    pub finished_recordings: Vec<String>,
    pub recording_error: Option<String>,
    /// Buffer kept in memory (true) or on disk
    pub in_ram: bool,
    pub buffer_mb: u32,
    /// Times the watchdog had to restart a frozen capture
    pub freezes: u32,
    /// "native" (WGC + NVENC) or "ffmpeg" (compatibility)
    pub engine: String,
}

#[derive(Clone)]
struct Gen {
    dir: PathBuf,
    width: i32,
    height: i32,
    encoder: String,
    fps: u32,
    /// RAM mode: finished segments live here instead of in `dir`.
    ram: Option<Arc<RamRing>>,
}

/// The RAM buffer for one generation: the newest finished segments, in order.
struct RamRing {
    segs: Mutex<VecDeque<(u64, Arc<Vec<u8>>)>>,
    cap: Mutex<usize>,
    stop: AtomicBool,
    /// When the newest finished segment arrived (freeze detection).
    last_push: Mutex<Instant>,
}

impl RamRing {
    fn push(&self, seq: u64, data: Vec<u8>) {
        *self.last_push.lock().unwrap() = Instant::now();
        let mut q = self.segs.lock().unwrap();
        q.push_back((seq, Arc::new(data)));
        let cap = *self.cap.lock().unwrap();
        while q.len() > cap {
            q.pop_front();
        }
    }
    fn bytes(&self) -> usize {
        self.segs.lock().unwrap().iter().map(|(_, d)| d.len()).sum()
    }
}

/// One finished segment, wherever it lives.
#[derive(Clone)]
enum Seg {
    File(PathBuf),
    Ram(Arc<Vec<u8>>),
}

#[derive(Clone)]
struct SegRef {
    /// Unique and stable for the segment's lifetime.
    id: String,
    seg: Seg,
}

fn segments_of(g: &Gen) -> Vec<SegRef> {
    match &g.ram {
        Some(r) => r
            .segs
            .lock()
            .unwrap()
            .iter()
            .map(|(n, d)| SegRef { id: format!("{}#{n}", g.dir.display()), seg: Seg::Ram(d.clone()) })
            .collect(),
        None => read_list(&g.dir)
            .into_iter()
            .map(|p| {
                let m = fs::metadata(&p).and_then(|m| m.modified()).ok();
                SegRef { id: format!("{}|{:?}", p.display(), m), seg: Seg::File(p) }
            })
            .collect(),
    }
}

impl Gen {
    /// Segments with the same key can be joined without re-encoding.
    fn key(&self) -> String {
        format!("{}x{}_{}_{}", self.width, self.height, self.encoder, self.fps)
    }
}

struct Recording {
    started: Instant,
    tmp: PathBuf,
    out: PathBuf,
    seen: HashSet<String>,
    seq: u32,
    last_disk_check: Instant,
}

enum Cmd {
    Restart,
    Shutdown(Sender<()>),
}

struct Running {
    child: Child,
    sig: String,
    log: Arc<Mutex<VecDeque<String>>>,
    started: Instant,
    dir: PathBuf,
    _audio: Vec<audio::Capture>,
    ram: Option<Arc<RamRing>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(r) = &self.ram {
            r.stop.store(true, Ordering::Relaxed);
        }
    }
}

struct Spawned {
    child: Child,
    log: Arc<Mutex<VecDeque<String>>>,
    audio: Vec<audio::Capture>,
    labels: Vec<String>,
    audio_error: Option<String>,
    ram: Option<Arc<RamRing>>,
}

pub struct Engine {
    status: Arc<Mutex<Status>>,
    gens: Arc<Mutex<Vec<Gen>>>,
    rec: Arc<Mutex<Option<Recording>>>,
    native: Arc<Mutex<NativeState>>,
    tx: Mutex<Sender<Cmd>>,
    buffer_root: PathBuf,
}

/// The native engine (WGC + NVENC, see native/): the running pipeline, the
/// buffers clips are cut from, and a recording in progress.
#[derive(Default)]
struct NativeState {
    run: Option<NativeRun>,
    /// Buffers of this run and earlier compatible runs, oldest first.
    runs: Vec<Arc<native::pipeline::Buffers>>,
    rec: Option<NativeRec>,
    failures: u32,
}

struct NativeRun {
    p: native::pipeline::Pipeline,
    sig: String,
    started: Instant,
    recording_part: Option<u32>,
}

struct NativeRec {
    tmp: PathBuf,
    out: PathBuf,
    started: Instant,
    next_part: u32,
    last_disk_check: Instant,
}

/// Native capture is skipped for the session after this many failed starts.
const NATIVE_MAX_FAILURES: u32 = 3;

impl Engine {
    pub fn start(settings: Arc<Mutex<Settings>>, buffer_root: PathBuf) -> Engine {
        let _ = fs::remove_dir_all(&buffer_root);
        let _ = fs::create_dir_all(&buffer_root);
        let (tx, rx) = channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let gens = Arc::new(Mutex::new(Vec::new()));
        let rec = Arc::new(Mutex::new(None));
        let native = Arc::new(Mutex::new(NativeState::default()));
        let (s, g, r, n, root) = (status.clone(), gens.clone(), rec.clone(), native.clone(), buffer_root.clone());
        thread::spawn(move || supervise(settings, s, g, r, n, root, rx));
        Engine { status, gens, rec, native, tx: Mutex::new(tx), buffer_root }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Hand over finished recordings once (the caller announces them).
    pub fn take_finished_recordings(&self) -> (Vec<String>, Option<String>) {
        let mut st = self.status.lock().unwrap();
        (std::mem::take(&mut st.finished_recordings), st.recording_error.take())
    }

    pub fn restart(&self) {
        let _ = self.tx.lock().unwrap().send(Cmd::Restart);
    }

    /// Stop ffmpeg cleanly before the app exits (finishing any recording first).
    pub fn shutdown(&self) {
        // Let clips being saved finish first: quitting (or updating) mid-save
        // would leave a half-written file.
        let until = std::time::Instant::now() + Duration::from_secs(90);
        while saves_in_progress() > 0 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.is_recording() {
            let _ = self.stop_recording();
        }
        // Stop the native pipeline (joins its capture thread).
        self.native.lock().unwrap().run = None;
        let (done_tx, done_rx) = channel();
        if self.tx.lock().unwrap().send(Cmd::Shutdown(done_tx)).is_ok() {
            let _ = done_rx.recv_timeout(Duration::from_secs(4));
        }
    }

    /// Write the last `seconds` of the buffer to `out` (an .mkv path).
    pub fn save_clip(&self, seconds: u32, out: &Path) -> Result<(), String> {
        let _saving = SaveGuard::new();
        check_space(out, MIN_FREE_FOR_CLIP)?;
        let native_runs = self.native.lock().unwrap().runs.clone();
        if !native_runs.is_empty() {
            return native::pipeline::save(&native_runs, seconds, out, &self.buffer_root);
        }
        let gens = self.gens.lock().unwrap().clone();
        let newest = gens.last().ok_or("The replay buffer isn't running yet.")?;
        wait_for_next_segment(newest);

        let need = seconds.div_ceil(SEG_SECONDS) as usize;
        let segs = collect_segments(&gens, need);
        if segs.is_empty() {
            return Err("Nothing buffered yet. Wait a couple of seconds.".into());
        }
        concat(&segs, out, &self.buffer_root)
    }

    pub fn is_recording(&self) -> bool {
        self.rec.lock().unwrap().is_some() || self.native.lock().unwrap().rec.is_some()
    }

    /// Start a full recording that will be saved to `out`.
    pub fn start_recording(&self, out: &Path) -> Result<(), String> {
        if self.is_recording() {
            return Err("Already recording.".into());
        }
        check_space(out, MIN_FREE_FOR_RECORDING)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let tmp = out
            .parent()
            .ok_or("Bad recording path")?
            .join(format!("{RECORDING_DIR_PREFIX}{stamp}"));
        fs::create_dir_all(&tmp).map_err(|e| format!("Can't create the recording folder: {e}"))?;

        // Native engine: the running pipeline streams straight into `tmp`.
        {
            let mut n = self.native.lock().unwrap();
            if n.run.is_some() || !n.runs.is_empty() {
                let mut rec = NativeRec { tmp: tmp.clone(), out: out.to_path_buf(), started: Instant::now(), next_part: 0, last_disk_check: Instant::now() };
                if let Some(run) = n.run.as_mut() {
                    run.p.begin_recording(&tmp, rec.next_part)?;
                    run.recording_part = Some(rec.next_part);
                    rec.next_part += 1;
                }
                n.rec = Some(rec);
                drop(n);
                self.restart_if_idle();
                return Ok(());
            }
        }

        let mut rec = self.rec.lock().unwrap();
        // Everything already in the buffer is from before "start": skip it.
        let mut seen = HashSet::new();
        for g in self.gens.lock().unwrap().iter() {
            seen.extend(segments_of(g).into_iter().map(|s| s.id));
        }
        *rec = Some(Recording {
            started: Instant::now(),
            tmp,
            out: out.to_path_buf(),
            seen,
            seq: 0,
            last_disk_check: Instant::now(),
        });
        drop(rec);
        self.restart_if_idle();
        Ok(())
    }

    /// Stop recording and join the pieces. Returns the saved file(s).
    pub fn stop_recording(&self) -> Result<Vec<String>, String> {
        let native_rec = {
            let mut n = self.native.lock().unwrap();
            let rec = n.rec.take();
            if rec.is_some()
                && let Some(run) = n.run.as_mut()
                && run.recording_part.take().is_some()
            {
                run.p.end_recording();
            }
            rec
        };
        if let Some(rec) = native_rec {
            native::pipeline::mux_recording_parts(&rec.tmp);
            return finalize(&rec.tmp, &rec.out, &self.buffer_root);
        }
        if let Some(newest) = self.gens.lock().unwrap().last().cloned() {
            wait_for_next_segment(&newest);
        }
        let mut guard = self.rec.lock().unwrap();
        let mut rec = guard.take().ok_or("Not recording.")?;
        copy_new_segments(&mut rec, &self.gens.lock().unwrap());
        drop(guard);
        finalize(&rec.tmp, &rec.out, &self.buffer_root)
    }

    fn restart_if_idle(&self) {
        // Nudge the supervisor so capture starts now if the buffer was paused.
        let _ = self.tx.lock().unwrap().send(Cmd::Restart);
    }
}

fn check_space(path: &Path, min: u64) -> Result<(), String> {
    match ffmpeg::free_bytes(path) {
        Some(free) if free < min => Err(format!(
            "Not enough disk space ({} MB free). Free some space or change the clips folder.",
            free / 1024 / 1024
        )),
        _ => Ok(()),
    }
}

/// Wait for the segment being written right now, so output includes "now".
fn wait_for_next_segment(newest: &Gen) {
    let last = || segments_of(newest).last().map(|s| s.id.clone());
    let before = last();
    let deadline = Instant::now() + Duration::from_millis(SEG_SECONDS as u64 * 1000 + 700);
    while Instant::now() < deadline && last() == before {
        thread::sleep(Duration::from_millis(100));
    }
}

/// Join segments into `out` without re-encoding. RAM segments are written
/// next to `out` only for the second this takes, then removed.
/// Join finished video files (same format) into `out` without re-encoding.
pub fn concat_files(files: &[PathBuf], out: &Path, scratch: &Path) -> Result<(), String> {
    let segs: Vec<Seg> = files.iter().cloned().map(Seg::File).collect();
    concat(&segs, out, scratch)
}

fn concat(segs: &[Seg], out: &Path, scratch: &Path) -> Result<(), String> {
    let parent = out.parent().ok_or("Bad output path")?;
    fs::create_dir_all(parent).map_err(|e| format!("Can't create clip folder: {e}"))?;
    let stamp = chrono::Local::now().format("%H%M%S%f");
    let staging = parent.join(format!(".rewind-save-{stamp}"));
    let mut paths = Vec::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        match s {
            Seg::File(p) => paths.push(p.clone()),
            Seg::Ram(d) => {
                fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
                let p = staging.join(format!("{i:06}.mkv"));
                fs::write(&p, d.as_slice()).map_err(|e| e.to_string())?;
                paths.push(p);
            }
        }
    }
    let list_path = scratch.join(format!("concat_{stamp}.txt"));
    let body: String = paths
        .iter()
        .map(|p| {
            let s = p.to_string_lossy().replace('\\', "/").replace('\'', r"'\''");
            format!("file '{s}'\n")
        })
        .collect();
    fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    fs::write(&list_path, body).map_err(|e| e.to_string())?;
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"]);
    args.push(list_path.clone().into());
    args.extend(ffmpeg::args(&["-map", "0", "-c", "copy"]));
    args.push(out.into());
    let result = ffmpeg::run("ffmpeg", &args);
    let _ = fs::remove_file(&list_path);
    let _ = fs::remove_dir_all(&staging);
    if result.is_err() {
        let _ = fs::remove_file(out);
    }
    result.map(|_| ())
}

fn read_list(dir: &Path) -> Vec<PathBuf> {
    fs::read_to_string(dir.join("list.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| dir.join(l))
        .collect()
}

/// The newest `max` finished segments, reaching back into older generations
/// only while they have the same resolution/codec (so they join cleanly).
fn collect_segments(gens: &[Gen], max: usize) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let Some(newest) = gens.last() else {
        return out;
    };
    for g in gens.iter().rev() {
        if g.key() != newest.key() {
            break;
        }
        let mut segs: Vec<Seg> = segments_of(g).into_iter().map(|s| s.seg).collect();
        segs.append(&mut out);
        out = segs;
        if out.len() >= max {
            break;
        }
    }
    if out.len() > max {
        out.drain(..out.len() - max);
    }
    out
}

/// Copy finished segments that the recording hasn't taken yet.
/// File names carry order and format: 000012_1920x1080_av1_60.mkv
fn copy_new_segments(rec: &mut Recording, gens: &[Gen]) {
    for g in gens {
        for s in segments_of(g) {
            if rec.seen.insert(s.id) {
                let dest = rec.tmp.join(format!("{:06}_{}.mkv", rec.seq, g.key()));
                let ok = match &s.seg {
                    Seg::File(p) => fs::copy(p, &dest).is_ok(),
                    Seg::Ram(d) => fs::write(&dest, d.as_slice()).is_ok(),
                };
                if ok {
                    rec.seq += 1;
                }
            }
        }
    }
}

/// Keep at most `cap` RAM segments across the joinable generations, and free
/// generations that can't be joined to the newest one anyway.
fn trim_ram(gens: &[Gen], cap: usize) {
    let Some(newest) = gens.last() else { return };
    let mut kept = 0usize;
    for g in gens.iter().rev() {
        let Some(r) = &g.ram else { continue };
        let mut q = r.segs.lock().unwrap();
        if g.key() != newest.key() {
            q.clear();
            continue;
        }
        let room = cap.saturating_sub(kept);
        while q.len() > room {
            q.pop_front();
        }
        kept += q.len();
    }
}

/// Receive segments from ffmpeg: each segment arrives as one TCP connection.
fn serve_ram(listener: TcpListener, ring: Arc<RamRing>) {
    let _ = listener.set_nonblocking(true);
    let mut seq = 0u64;
    while !ring.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut sock, _)) => {
                let _ = sock.set_nonblocking(false);
                let mut buf = Vec::with_capacity(4 << 20);
                // An interrupted segment errors out here and is dropped.
                if sock.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                    ring.push(seq, buf);
                    seq += 1;
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
}

/// Join a recording folder into `out` (or "out (part N)" when the resolution
/// changed mid-recording), then delete the folder.
pub fn finalize(tmp: &Path, out: &Path, scratch: &Path) -> Result<Vec<String>, String> {
    let mut files: Vec<PathBuf> = fs::read_dir(tmp)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mkv"))
        .collect();
    files.sort();
    if files.is_empty() {
        let _ = fs::remove_dir_all(tmp);
        return Err("The recording was too short to save.".into());
    }
    let key = |p: &PathBuf| {
        p.file_stem()
            .map(|s| s.to_string_lossy().split_once('_').map(|(_, k)| k.to_string()).unwrap_or_default())
            .unwrap_or_default()
    };
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for f in files {
        match groups.last_mut() {
            Some(g) if key(&g[0]) == key(&f) => g.push(f),
            _ => groups.push(vec![f]),
        }
    }
    let mut saved = Vec::new();
    let n = groups.len();
    for (i, g) in groups.iter().enumerate() {
        let target = if n == 1 {
            out.to_path_buf()
        } else {
            let stem = out.file_stem().unwrap_or_default().to_string_lossy();
            out.with_file_name(format!("{stem} (part {}).mkv", i + 1))
        };
        let segs: Vec<Seg> = g.iter().cloned().map(Seg::File).collect();
        concat(&segs, &target, scratch)?;
        saved.push(target.to_string_lossy().into_owned());
    }
    let _ = fs::remove_dir_all(tmp);
    Ok(saved)
}

static SAVES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Clips being written right now.
pub fn saves_in_progress() -> usize {
    SAVES.load(std::sync::atomic::Ordering::SeqCst)
}

/// Counts a save while it runs (also if it fails or panics).
struct SaveGuard;

impl SaveGuard {
    fn new() -> SaveGuard {
        SAVES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        SaveGuard
    }
}

impl Drop for SaveGuard {
    fn drop(&mut self) {
        SAVES.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Turn leftover recording folders (from a crash) into files.
pub fn recover_recordings(clips_dir: &Path, scratch: &Path) -> Vec<String> {
    let mut saved = Vec::new();
    let mut dirs = Vec::new();
    for top in [clips_dir.to_path_buf()]
        .into_iter()
        .chain(fs::read_dir(clips_dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()))
    {
        for e in fs::read_dir(&top).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && name.starts_with(RECORDING_DIR_PREFIX) {
                dirs.push((top.clone(), name, e.path()));
            }
        }
    }
    for (parent, name, dir) in dirs {
        let stamp = name.trim_start_matches(RECORDING_DIR_PREFIX);
        let out = parent.join(format!("Recovered recording {stamp}.mkv"));
        // Native recordings leave raw parts: turn them into MKVs first.
        native::pipeline::mux_recording_parts(&dir);
        if let Ok(files) = finalize(&dir, &out, scratch) {
            saved.extend(files);
        }
    }
    saved
}

/// Clip size for a monitor: same as the monitor, or scaled to `height`
/// keeping the aspect ratio (even numbers, as encoders need).
pub fn output_size(m: &monitors::Monitor, height: u32) -> (i32, i32) {
    if height == 0 || height as i32 == m.height || m.height <= 0 {
        return (m.width, m.height);
    }
    let h = height as i32;
    let w = ((m.width as f64 * h as f64 / m.height as f64) / 2.0).round() as i32 * 2;
    (w, h)
}

/// The part of the monitor that's recorded, in pixels: (x, y, w, h).
pub fn area_px(s: &Settings, m: &monitors::Monitor) -> (i32, i32, i32, i32) {
    match s.capture_area {
        Some(a) => a.pixels(m.width, m.height),
        None => (0, 0, m.width, m.height),
    }
}

/// Clip size for these settings: the recorded area (or monitor), scaled to
/// the clip height. A recording area is never scaled up.
pub fn capture_size(s: &Settings, m: &monitors::Monitor) -> (i32, i32) {
    let (_, _, w, h) = area_px(s, m);
    if s.capture_area.is_some() && (s.output_height == 0 || s.output_height as i32 >= h) {
        return (w, h);
    }
    output_size(&monitors::Monitor { width: w, height: h, ..m.clone() }, s.output_height)
}

/// Don't judge a capture that has only just started.
const FREEZE_GRACE: Duration = Duration::from_secs(10);
/// Segments finish every 2 s; 3 missed in a row means the grabber is stuck.
const FREEZE_AFTER: Duration = Duration::from_secs(SEG_SECONDS as u64 * 3 + 1);

/// How long ago the running capture last finished a segment.
fn segment_age(r: &Running) -> Duration {
    match &r.ram {
        Some(ring) => ring.last_push.lock().unwrap().elapsed(),
        None => fs::metadata(r.dir.join("list.txt"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or_else(|| r.started.elapsed()),
    }
}

fn segs_needed(buffer_seconds: u32) -> u32 {
    buffer_seconds.div_ceil(SEG_SECONDS) + 1
}

/// What the native engine reports back for the status panel.
struct NativeTick {
    running: bool,
    buffered: u32,
    ram_mb: u32,
    rec_secs: Option<u32>,
}

/// One supervisor step for the native engine.
#[allow(clippy::too_many_arguments)]
fn supervise_native(
    s: &Settings,
    picked: Option<&monitors::Monitor>,
    sig: &str,
    want: bool,
    native: &Arc<Mutex<NativeState>>,
    status: &Arc<Mutex<Status>>,
    root: &Path,
    retry_at: &mut Instant,
) -> NativeTick {
    // 1. Is the running pipeline still healthy and still what's wanted?
    {
        let mut n = native.lock().unwrap();
        if let Some(run) = n.run.as_ref() {
            let problem: Option<Option<String>> = if !want {
                Some(None)
            } else if run.sig != sig {
                crate::log!("[rewind] restarting native capture: settings or displays/devices changed");
                Some(None)
            } else if let Some(e) = run.p.failed() {
                Some(Some(format!("Capture restarted: {e}")))
            } else if run.started.elapsed() > FREEZE_GRACE && run.p.frame_age() > Duration::from_secs(3) {
                Some(Some("Capture froze and was restarted automatically.".into()))
            } else {
                None
            };
            if let Some(err) = problem {
                let run = n.run.take();
                drop(n);
                drop(run); // stops the thread; an open recording part is closed
                if let Some(e) = err {
                    crate::log!("[rewind] native capture: {e}");
                    let mut st = status.lock().unwrap();
                    st.last_error = Some(e);
                    st.restarts += 1;
                    *retry_at = Instant::now() + Duration::from_secs(1);
                }
            }
        }
    }

    // 2. Start one if needed.
    let need_start = want && Instant::now() >= *retry_at && native.lock().unwrap().run.is_none();
    if need_start && let Some(m) = picked {
        let (w, h) = capture_size(s, m);
        let key = format!("{w}x{h}_{}_{}", s.encoder, s.fps);
        match native::pipeline::Pipeline::start(s, m, w as u32, h as u32, key.clone()) {
            Ok(p) => {
                let mut n = native.lock().unwrap();
                n.failures = 0;
                if n.runs.last().is_some_and(|b| b.key != key) {
                    n.runs.clear(); // different format: older footage can't be joined
                }
                n.runs.push(p.buffers.clone());
                while n.runs.len() > 3 {
                    n.runs.remove(0);
                }
                let mut st = status.lock().unwrap();
                st.monitor = Some(m.label());
                (st.width, st.height) = (w, h);
                st.audio = p.labels.clone();
                st.audio_error = p.audio_error.clone();
                drop(st);
                n.run = Some(NativeRun { p, sig: sig.to_string(), started: Instant::now(), recording_part: None });
            }
            Err(e) => {
                let mut n = native.lock().unwrap();
                n.failures += 1;
                let msg = if n.failures >= NATIVE_MAX_FAILURES {
                    format!("Native capture couldn't start ({e}). Using compatibility capture instead.")
                } else {
                    format!("Native capture couldn't start: {e}")
                };
                crate::log!("[rewind] {msg}");
                status.lock().unwrap().last_error = Some(msg);
                *retry_at = Instant::now() + Duration::from_secs(2);
            }
        }
    }

    // 3. Recording: a fresh pipeline joins a recording in progress as a new
    //    part; stop it automatically when the disk runs low.
    let mut n = native.lock().unwrap();
    let mut auto_stop: Option<String> = None;
    let NativeState { run, rec, .. } = &mut *n;
    if let Some(rec) = rec.as_mut() {
        if let Some(run) = run.as_mut()
            && run.recording_part.is_none()
            && run.p.begin_recording(&rec.tmp, rec.next_part).is_ok()
        {
            run.recording_part = Some(rec.next_part);
            rec.next_part += 1;
        }
        if rec.last_disk_check.elapsed() > Duration::from_secs(5) {
            rec.last_disk_check = Instant::now();
            if let Err(e) = check_space(&rec.tmp, MIN_FREE_FOR_RECORDING) {
                auto_stop = Some(format!("Recording stopped: {e}"));
            }
        }
    }
    if let Some(why) = auto_stop {
        let r = n.rec.take().unwrap();
        if let Some(run) = n.run.as_mut()
            && run.recording_part.take().is_some()
        {
            run.p.end_recording();
        }
        drop(n);
        native::pipeline::mux_recording_parts(&r.tmp);
        let result = finalize(&r.tmp, &r.out, root);
        let mut st = status.lock().unwrap();
        if let Ok(files) = result {
            st.finished_recordings.extend(files);
        }
        st.recording_error = Some(why);
        n = native.lock().unwrap();
    }

    // 4. Numbers for the status panel.
    let newest_key = n.runs.last().map(|b| b.key.clone());
    let mut frames = 0usize;
    let mut bytes = 0usize;
    let mut fps = s.fps.max(1);
    for b in n.runs.iter().rev() {
        let v = b.video.lock().unwrap();
        bytes += v.packets.iter().map(|p| p.data.len()).sum::<usize>();
        if Some(&b.key) == newest_key.as_ref() {
            frames += v.packets.len();
            fps = v.fps.max(1);
        }
        drop(v);
        bytes += b.audio.iter().map(|t| t.ring.lock().unwrap().bytes()).sum::<usize>();
    }
    NativeTick {
        running: n.run.is_some(),
        buffered: ((frames as u32) / fps).min(s.buffer_seconds),
        ram_mb: (bytes / (1024 * 1024)) as u32,
        rec_secs: n.rec.as_ref().map(|r| r.started.elapsed().as_secs() as u32),
    }
}

fn supervise(
    settings: Arc<Mutex<Settings>>,
    status: Arc<Mutex<Status>>,
    gens: Arc<Mutex<Vec<Gen>>>,
    rec: Arc<Mutex<Option<Recording>>>,
    native: Arc<Mutex<NativeState>>,
    root: PathBuf,
    rx: Receiver<Cmd>,
) {
    let mut cur: Option<Running> = None;
    let mut gen_no = 0u32;
    let mut retry_at = Instant::now();

    loop {
        let mut s = settings.lock().unwrap().clone();
        // "auto", or a choice this GPU can't encode, becomes what it can (AV1 needs an RTX 40).
        s.encoder = crate::native::nvenc::effective_encoder(&s.encoder).to_string();
        let use_native = s.capture_engine == "native" && native.lock().unwrap().failures < NATIVE_MAX_FAILURES;
        let recording = rec.lock().unwrap().is_some() || native.lock().unwrap().rec.is_some();
        // A recording keeps capture running even if the replay buffer is paused.
        let want = s.buffer_enabled || recording;
        let mons = monitors::list();
        let picked = monitors::pick(&s.monitor_mode, &s.fixed_monitor, &mons);
        let devices = if s.desktop_audio || s.mic_enabled { audio::default_devices() } else { (None, None) };
        // Anything in here changing means "restart capture" (incl. a new default headset).
        let sig = format!(
            "{}|{:?}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}",
            s.buffer_in_ram,
            mons,
            picked.as_ref().map(|m| m.index),
            s.encoder,
            s.quality,
            s.fps,
            s.draw_cursor,
            s.buffer_seconds * 10_000 + s.output_height,
            s.desktop_audio,
            s.discord_track,
            s.mic_enabled,
            s.mic_mono,
            s.mic_device,
            devices,
            s.capture_area,
        );

        // Native engine (default): run it and skip the FFmpeg engine entirely.
        let native_tick = if use_native {
            if let Some(r) = cur.take() {
                stop(r);
            }
            Some(supervise_native(&s, picked.as_ref(), &sig, want, &native, &status, &root, &mut retry_at))
        } else {
            let mut n = native.lock().unwrap();
            if n.run.is_some() || !n.runs.is_empty() {
                n.run = None;
                n.runs.clear();
            }
            None
        };

        if !use_native && let Some(r) = cur.as_mut() {
            match r.child.try_wait() {
                Ok(Some(code)) => {
                    let log = r.log.lock().unwrap();
                    let tail: Vec<String> = log.iter().rev().take(3).rev().cloned().collect();
                    let mut st = status.lock().unwrap();
                    st.last_error = Some(format!("ffmpeg stopped ({code}). {}", tail.join(" ")));
                    crate::log!("[rewind] capture stopped: {:?}", st.last_error);
                    st.restarts += 1;
                    drop(st);
                    drop(log);
                    cur = None;
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
                Ok(None) if !want || r.sig != sig => {
                    crate::log!("[rewind] restarting capture: settings or displays/devices changed");
                    stop(cur.take().unwrap())
                }
                // Watchdog: ffmpeg is alive but no new video is coming out
                // (e.g. a UAC prompt / lock screen hung the screen grabber).
                // Restart it rather than keep re-saving old footage.
                Ok(None) if r.started.elapsed() > FREEZE_GRACE && segment_age(r) > FREEZE_AFTER => {
                    let age = segment_age(r).as_secs();
                    crate::log!("[rewind] capture froze (no new video for {age} s), restarting");
                    let mut st = status.lock().unwrap();
                    st.last_error = Some(format!("Capture froze for {age} s and was restarted automatically."));
                    st.restarts += 1;
                    st.freezes += 1;
                    drop(st);
                    stop(cur.take().unwrap());
                    retry_at = Instant::now() + Duration::from_secs(1);
                }
                _ => {}
            }
        }

        if !use_native && cur.is_none() && want && Instant::now() >= retry_at {
            match picked {
                Some(m) => {
                    gen_no += 1;
                    let mut generation = Gen {
                        dir: root.join(format!("gen_{gen_no:04}")),
                        width: capture_size(&s, &m).0,
                        height: capture_size(&s, &m).1,
                        encoder: s.encoder.clone(),
                        fps: s.fps,
                        ram: None,
                    };
                    match spawn(&s, &m, &generation) {
                        Ok(sp) => {
                            generation.ram = sp.ram.clone();
                            let mut g = gens.lock().unwrap();
                            g.push(generation);
                            while g.len() > 4 {
                                let old = g.remove(0);
                                let _ = fs::remove_dir_all(&old.dir);
                            }
                            drop(g);
                            let mut st = status.lock().unwrap();
                            st.monitor = Some(m.label());
                            (st.width, st.height) = capture_size(&s, &m);
                            st.audio = sp.labels;
                            st.audio_error = sp.audio_error;
                            cur = Some(Running {
                                child: sp.child,
                                sig: sig.clone(),
                                log: sp.log,
                                started: Instant::now(),
                                dir: gens.lock().unwrap().last().map(|g| g.dir.clone()).unwrap_or_default(),
                                _audio: sp.audio,
                                ram: sp.ram,
                            });
                        }
                        Err(e) => {
                            status.lock().unwrap().last_error = Some(e);
                            retry_at = Instant::now() + Duration::from_secs(3);
                        }
                    }
                }
                None => status.lock().unwrap().last_error = Some("No monitor found.".into()),
            }
        }

        // Recording: pull finished segments out of the ring; stop on low disk.
        let mut rec_secs = 0;
        let mut auto_stopped: Option<(Recording, String)> = None;
        {
            let mut guard = rec.lock().unwrap();
            if let Some(r) = guard.as_mut() {
                copy_new_segments(r, &gens.lock().unwrap());
                rec_secs = r.started.elapsed().as_secs() as u32;
                if r.last_disk_check.elapsed() > Duration::from_secs(5) {
                    r.last_disk_check = Instant::now();
                    if let Err(e) = check_space(&r.tmp, MIN_FREE_FOR_RECORDING) {
                        auto_stopped = Some((guard.take().unwrap(), format!("Recording stopped: {e}")));
                    }
                }
            }
        }
        if let Some((r, why)) = auto_stopped {
            let result = finalize(&r.tmp, &r.out, &root);
            let mut st = status.lock().unwrap();
            if let Ok(files) = result {
                st.finished_recordings.extend(files);
            }
            st.recording_error = Some(why);
        }

        {
            let g = gens.lock().unwrap();
            trim_ram(&g, segs_needed(s.buffer_seconds) as usize + 1);
            let buffered = if cur.is_some() {
                let n = collect_segments(&g, usize::MAX).len() as u32;
                (n * SEG_SECONDS).min(s.buffer_seconds)
            } else {
                0
            };
            let ram_bytes: usize = g.iter().filter_map(|x| x.ram.as_ref()).map(|r| r.bytes()).sum();
            drop(g);
            let game = games::foreground_game();
            let mut st = status.lock().unwrap();
            st.enabled = s.buffer_enabled;
            st.encoder = s.encoder.clone();
            st.fps = s.fps;
            st.buffer_seconds = s.buffer_seconds;
            st.game = game;
            match &native_tick {
                Some(t) => {
                    st.running = t.running;
                    st.buffered_seconds = t.buffered;
                    st.recording = t.rec_secs.is_some();
                    st.recording_seconds = t.rec_secs.unwrap_or(0);
                    st.in_ram = true;
                    st.buffer_mb = t.ram_mb;
                    st.engine = "native".into();
                }
                None => {
                    st.running = cur.is_some();
                    st.buffered_seconds = buffered;
                    st.recording = rec.lock().unwrap().is_some();
                    st.recording_seconds = rec_secs;
                    st.in_ram = s.buffer_in_ram;
                    st.buffer_mb = (ram_bytes / (1024 * 1024)) as u32;
                    st.engine = "ffmpeg".into();
                }
            }
            if !st.running {
                st.monitor = None;
                st.audio.clear();
            }
        }

        match rx.recv_timeout(Duration::from_millis(1000)) {
            Ok(Cmd::Restart) => {
                // Only a real restart if something is running and nothing is recording.
                if !rec.lock().unwrap().is_some()
                    && let Some(r) = cur.take()
                {
                    stop(r);
                }
                retry_at = Instant::now();
            }
            Ok(Cmd::Shutdown(done)) => {
                if let Some(r) = cur.take() {
                    stop(r);
                }
                native.lock().unwrap().run = None;
                let _ = fs::remove_dir_all(&root);
                let _ = done.send(());
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(r) = cur.take() {
                    stop(r);
                }
                return;
            }
        }
    }
}

fn spawn(s: &Settings, m: &monitors::Monitor, generation: &Gen) -> Result<Spawned, String> {
    // RAM mode: ffmpeg sends each finished segment to us over localhost,
    // so the replay buffer never touches the disk.
    let ram = if s.buffer_in_ram {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let ring = Arc::new(RamRing {
            segs: Mutex::new(VecDeque::new()),
            cap: Mutex::new(segs_needed(s.buffer_seconds) as usize + 1),
            stop: AtomicBool::new(false),
            last_push: Mutex::new(Instant::now()),
        });
        let r = ring.clone();
        thread::spawn(move || serve_ram(listener, r));
        Some((ring, port))
    } else {
        fs::create_dir_all(&generation.dir).map_err(|e| e.to_string())?;
        None
    };

    // Audio first, so its sockets are listening when ffmpeg connects.
    let mut caps: Vec<(audio::Capture, &str, &str, String)> = Vec::new();
    let mut audio_errors = Vec::new();
    let desktop = if s.desktop_audio && s.discord_track { audio::Source::DesktopNoDiscord } else { audio::Source::Desktop };
    if s.desktop_audio {
        match audio::start(desktop, AUDIO_DELAY_SECS) {
            Ok(c) => {
                let label = format!("Desktop: {}", c.device);
                caps.push((c, "Desktop audio", "192k", label));
            }
            Err(e) => audio_errors.push(format!("Desktop audio: {e}")),
        }
    }
    if s.mic_enabled {
        match audio::start(audio::Source::Mic { device: s.mic_device.clone(), mono: s.mic_mono }, AUDIO_DELAY_SECS) {
            Ok(c) => {
                let label = format!("Mic{}: {}", if c.channels == 1 { " (mono)" } else { "" }, c.device);
                caps.push((c, "Microphone", "96k", label));
            }
            Err(e) => audio_errors.push(format!("Mic: {e}")),
        }
    }
    if s.desktop_audio && s.discord_track {
        match audio::start(audio::Source::Discord, AUDIO_DELAY_SECS) {
            Ok(c) => caps.push((c, "Discord audio", "160k", "Discord (own track)".to_string())),
            Err(e) => audio_errors.push(format!("Discord: {e}")),
        }
    }

    let per = segs_needed(s.buffer_seconds);
    let codec = match s.encoder.as_str() {
        "h264" => "h264_nvenc",
        "hevc" => "hevc_nvenc",
        _ => "av1_nvenc",
    };
    let mut src = format!(
        "ddagrab=output_idx={}:framerate={}:draw_mouse={}",
        m.index,
        s.fps,
        if s.draw_cursor { 1 } else { 0 }
    );
    let (ax, ay, aw, ah) = area_px(s, m);
    if s.capture_area.is_some() {
        src.push_str(&format!(":offset_x={ax}:offset_y={ay}:video_size={aw}x{ah}"));
    }
    let (w, h) = capture_size(s, m);
    if (w, h) != (aw, ah) {
        // Native size stays 100% on the GPU. Other sizes are scaled on the CPU
        // (about 1-5% of a modern CPU): this FFmpeg's GPU scalers can't take
        // desktop-capture frames.
        src.push_str(&format!(",hwdownload,format=bgra,scale={w}:{h}:flags=bicubic,format=nv12"));
    }
    let gop = (s.fps * SEG_SECONDS).to_string();
    let cq = s.quality.to_string();
    let seg = SEG_SECONDS.to_string();
    let wrap = (per + 4).to_string();
    let list_size = (per + 2).to_string();

    let mut cmd = ffmpeg::cmd("ffmpeg");
    if ram.is_none() {
        cmd.current_dir(&generation.dir);
    }
    cmd.args(["-hide_banner", "-loglevel", "warning", "-f", "lavfi", "-i", &src]);
    for (c, _, _, _) in &caps {
        // Raw PCM with a known format: skip ffmpeg's multi-second stream probing.
        cmd.args(["-probesize", "32", "-analyzeduration", "0"])
            .args(["-fflags", "nobuffer", "-f", "f32le"])
            .args(["-ar", &c.rate.to_string(), "-ac", &c.channels.to_string()])
            .args(["-i", &c.url]);
    }
    cmd.args(["-map", "0:v"]);
    for (i, (_, title, bitrate, _)) in caps.iter().enumerate() {
        cmd.args(["-map", &format!("{}:a", i + 1)])
            .arg(format!("-b:a:{i}"))
            .arg(bitrate)
            .arg(format!("-metadata:s:a:{i}"))
            .arg(format!("title={title}"));
    }
    if !caps.is_empty() {
        cmd.args(["-c:a", "aac", "-ar", "48000"]).args(crate::ffmpeg::AAC_SAFE);
    }
    cmd.args(["-c:v", codec, "-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", &cq, "-b:v", "0"])
        .args(["-g", &gop])
        .args(["-f", "segment", "-segment_time", &seg, "-segment_format", "matroska"])
        .args(["-reset_timestamps", "1"]);
    match &ram {
        Some((_, port)) => {
            cmd.arg(format!("tcp://127.0.0.1:{port}/seg_%05d.mkv"));
        }
        None => {
            cmd.args(["-segment_wrap", &wrap, "-segment_list", "list.txt"])
                .args(["-segment_list_size", &list_size, "-segment_list_type", "flat", "seg_%03d.mkv"]);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    ffmpeg::adopt(&child);

    let log = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(err) = child.stderr.take() {
        let log = log.clone();
        thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let mut q = log.lock().unwrap();
                q.push_back(line);
                if q.len() > 30 {
                    q.pop_front();
                }
            }
        });
    }
    let labels = caps.iter().map(|(_, _, _, l)| l.clone()).collect();
    Ok(Spawned {
        child,
        log,
        audio: caps.into_iter().map(|(c, _, _, _)| c).collect(),
        labels,
        audio_error: if audio_errors.is_empty() { None } else { Some(audio_errors.join(" · ")) },
        ram: ram.map(|(r, _)| r),
    })
}

/// Ask ffmpeg to finish its current segment ("q"), then force it if needed.
fn stop(mut r: Running) {
    if let Some(mut stdin) = r.child.stdin.take() {
        let _ = stdin.write_all(b"q");
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = r.child.try_wait() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = r.child.kill();
    let _ = r.child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_gen(dir: &Path, w: i32) -> Gen {
        Gen { dir: dir.to_path_buf(), width: w, height: 1080, encoder: "av1".into(), fps: 60, ram: None }
    }

    fn file_name(s: &Seg) -> String {
        match s {
            Seg::File(p) => p.file_name().unwrap().to_string_lossy().into_owned(),
            Seg::Ram(_) => "<ram>".into(),
        }
    }

    #[test]
    fn watchdog_sees_a_frozen_capture() {
        // A "capture" that started 20 s ago and whose last segment is 9 s old.
        let ring = Arc::new(RamRing {
            segs: Mutex::new(VecDeque::new()),
            cap: Mutex::new(10),
            stop: AtomicBool::new(false),
            last_push: Mutex::new(Instant::now() - Duration::from_secs(9)),
        });
        let child = std::process::Command::new("cmd").args(["/c", "exit"]).spawn().unwrap();
        let r = Running {
            child,
            sig: String::new(),
            log: Arc::new(Mutex::new(VecDeque::new())),
            started: Instant::now() - Duration::from_secs(20),
            dir: PathBuf::new(),
            _audio: Vec::new(),
            ram: Some(ring.clone()),
        };
        assert!(r.started.elapsed() > FREEZE_GRACE && segment_age(&r) > FREEZE_AFTER, "frozen capture must trip the watchdog");
        // A healthy one (segment just arrived) must not.
        ring.push(1, vec![0]);
        assert!(segment_age(&r) < FREEZE_AFTER);
    }

    #[test]
    fn output_size_keeps_aspect_and_even_numbers() {
        let mon = |w, h| monitors::Monitor { index: 0, name: "d".into(), x: 0, y: 0, width: w, height: h, primary: true };
        assert_eq!(output_size(&mon(1920, 1080), 0), (1920, 1080));
        assert_eq!(output_size(&mon(1920, 1080), 1080), (1920, 1080));
        assert_eq!(output_size(&mon(1920, 1080), 720), (1280, 720));
        assert_eq!(output_size(&mon(1920, 1080), 2160), (3840, 2160));
        assert_eq!(output_size(&mon(3840, 2160), 1440), (2560, 1440));
        assert_eq!(output_size(&mon(3440, 1440), 1080), (2580, 1080)); // ultrawide
        assert_eq!(output_size(&mon(1366, 768), 720), (1280, 720));
        // A recording area: its own size, never scaled up.
        let area = crate::settings::Area { x: 0.25, y: 0.0, w: 0.5, h: 1.0 };
        let s = Settings { capture_area: Some(area), ..Default::default() };
        assert_eq!(area_px(&s, &mon(1920, 1080)), (480, 0, 960, 1080));
        assert_eq!(capture_size(&s, &mon(1920, 1080)), (960, 1080));
        assert_eq!(capture_size(&Settings { output_height: 720, ..s.clone() }, &mon(1920, 1080)), (640, 720));
        assert_eq!(capture_size(&Settings { output_height: 1440, ..s }, &mon(1920, 1080)), (960, 1080));
    }

    #[test]
    fn ram_ring_caps_and_trims_across_generations() {
        let ring = |n: usize| {
            Arc::new(RamRing {
                segs: Mutex::new(VecDeque::new()),
                cap: Mutex::new(n),
                stop: AtomicBool::new(false),
                last_push: Mutex::new(Instant::now()),
            })
        };
        let (a, b) = (ring(10), ring(10));
        for i in 0..15 {
            a.push(i, vec![0u8; 100]);
        }
        assert_eq!(a.segs.lock().unwrap().len(), 10, "ring keeps only its cap");
        for i in 0..6 {
            b.push(i, vec![0u8; 100]);
        }
        let mut g1 = mk_gen(Path::new("g1"), 1920);
        g1.ram = Some(a.clone());
        let mut g2 = mk_gen(Path::new("g2"), 1920);
        g2.ram = Some(b.clone());
        trim_ram(&[g1.clone(), g2.clone()], 8);
        assert_eq!(b.segs.lock().unwrap().len(), 6, "newest generation kept whole");
        assert_eq!(a.segs.lock().unwrap().len(), 2, "older one trimmed to fit");
        assert_eq!(collect_segments(&[g1.clone(), g2.clone()], usize::MAX).len(), 8);
        // A different-format generation in front frees the old ones.
        let mut g3 = mk_gen(Path::new("g3"), 2560);
        g3.ram = Some(ring(10));
        trim_ram(&[g1, g2, g3], 8);
        assert_eq!(a.segs.lock().unwrap().len() + b.segs.lock().unwrap().len(), 0);
    }

    #[test]
    fn collect_takes_newest_and_stops_at_format_change() {
        let base = std::env::temp_dir().join(format!("rewind-test-collect-{}", std::process::id()));
        let (a, b, c) = (base.join("a"), base.join("b"), base.join("c"));
        for (d, names) in [(&a, "x1\nx2\n"), (&b, "y1\ny2\ny3\n"), (&c, "z1\n")] {
            fs::create_dir_all(d).unwrap();
            fs::write(d.join("list.txt"), names).unwrap();
        }
        // a is 1440 wide (different format), b and c are 1920.
        let gens = vec![mk_gen(&a, 1440), mk_gen(&b, 1920), mk_gen(&c, 1920)];
        let all = collect_segments(&gens, usize::MAX);
        let names: Vec<String> = all.iter().map(file_name).collect();
        assert_eq!(names, ["y1", "y2", "y3", "z1"]);
        let two = collect_segments(&gens, 2);
        assert_eq!(two.len(), 2);
        assert_eq!(file_name(&two[1]), "z1");
        let _ = fs::remove_dir_all(&base);
    }

    /// Real capture on this PC: buffer → clip, recording, crash recovery.
    /// Run with: cargo test live_ -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_native() {
        live_run("native", true);
    }

    #[test]
    #[ignore]
    fn live_ffmpeg_ram_buffer() {
        live_run("ffmpeg", true);
    }

    #[test]
    #[ignore]
    fn live_ffmpeg_disk_buffer() {
        live_run("ffmpeg", false);
    }

    fn live_run(engine_kind: &str, in_ram: bool) {
        crate::ffmpeg::init(None);
        let base = std::env::temp_dir().join(format!("rewind-test-live-{engine_kind}-{in_ram}-{}", std::process::id()));
        let clips = base.join("clips");
        let s = Settings {
            clips_dir: clips.to_string_lossy().into(),
            buffer_seconds: 20,
            buffer_in_ram: in_ram,
            capture_engine: engine_kind.into(),
            ..Default::default()
        };
        let engine = Engine::start(Arc::new(Mutex::new(s)), base.join("buffer"));
        let probe = |p: &Path| crate::library::probe(&p.to_string_lossy()).unwrap();

        // 1. Buffer fills.
        let t = Instant::now();
        while engine.status().buffered_seconds < 8 {
            assert!(t.elapsed() < Duration::from_secs(40), "buffer never filled: {:?}", engine.status().last_error);
            thread::sleep(Duration::from_millis(250));
        }
        let st = engine.status();
        println!("[{}] capturing {:?} {}x{} audio={:?} err={:?}", st.engine, st.monitor, st.width, st.height, st.audio, st.audio_error);
        assert_eq!(st.engine, engine_kind, "ran on the wrong engine: {:?}", st.last_error);

        // 2. Save a 6 s clip.
        let clip = clips.join("Desktop").join("clip.mkv");
        engine.save_clip(6, &clip).unwrap();
        let p = probe(&clip);
        println!("clip: {:.2}s {} audio tracks {}", p.duration, p.codec, p.audio_tracks);
        assert!((5.5..=8.5).contains(&p.duration), "clip length {}", p.duration);
        assert_eq!(p.codec, "av1");
        assert_eq!(p.audio_tracks as usize, st.audio.len());

        // 3. Record ~7 s.
        let rec_out = clips.join("Desktop").join("recording.mkv");
        engine.start_recording(&rec_out).unwrap();
        assert!(engine.start_recording(&rec_out).is_err(), "no double start");
        thread::sleep(Duration::from_secs(7));
        assert!(engine.status().recording);
        let files = engine.stop_recording().unwrap();
        assert_eq!(files.len(), 1);
        let r = probe(Path::new(&files[0]));
        println!("recording: {:.2}s", r.duration);
        // Never shorter than the time recorded (it may start up to ~1 s early).
        assert!((7.0..=11.5).contains(&r.duration), "recording length {}", r.duration);
        assert!(engine.stop_recording().is_err(), "no double stop");
        let leftovers = fs::read_dir(clips.join("Desktop")).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(RECORDING_DIR_PREFIX)).count();
        assert_eq!(leftovers, 0, "temp recording folder cleaned up");

        // RAM buffers must not have written the buffer to disk.
        if in_ram {
            let on_disk = fs::read_dir(base.join("buffer")).map(|d| d.count()).unwrap_or(0);
            println!("RAM mode: {on_disk} items in buffer folder, {} MB in memory", engine.status().buffer_mb);
            assert_eq!(on_disk, 0, "RAM buffer wrote to disk");
        }

        // 4. A crashed recording (pieces left behind) is recovered.
        let crashed = clips.join("Desktop").join(format!("{RECORDING_DIR_PREFIX}crash"));
        fs::create_dir_all(&crashed).unwrap();
        let expect = if engine_kind == "native" {
            // Start a recording, then "crash": abandon it without finishing.
            engine.start_recording(&crashed.with_extension("mkv")).unwrap();
            let tmp = engine.native.lock().unwrap().rec.as_ref().unwrap().tmp.clone();
            thread::sleep(Duration::from_secs(4));
            let mut n = engine.native.lock().unwrap();
            n.rec = None;
            n.run = None; // the pipeline closes its files as the app would on exit
            drop(n);
            fs::remove_dir_all(&crashed).unwrap();
            fs::rename(&tmp, &crashed).unwrap();
            3.0..=5.5
        } else {
            let g = engine.gens.lock().unwrap().last().cloned().unwrap();
            for (i, s) in segments_of(&g).iter().take(3).enumerate() {
                let dest = crashed.join(format!("{i:06}_{}.mkv", g.key()));
                match &s.seg {
                    Seg::File(p) => fs::copy(p, &dest).map(|_| ()).unwrap(),
                    Seg::Ram(d) => fs::write(&dest, d.as_slice()).unwrap(),
                }
            }
            5.0..=6.5
        };
        let recovered = recover_recordings(&clips, &base.join("scratch"));
        assert_eq!(recovered.len(), 1);
        let rv = probe(Path::new(&recovered[0]));
        println!("recovered: {:.2}s, {} audio tracks", rv.duration, rv.audio_tracks);
        assert!(expect.contains(&rv.duration), "recovered length {}", rv.duration);
        assert!(!crashed.exists());

        engine.shutdown();
        let _ = fs::remove_dir_all(&base);
    }
}
