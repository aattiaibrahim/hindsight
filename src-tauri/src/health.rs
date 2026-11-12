//! Clip health check: finds clips that are damaged (like an NVIDIA Instant
//! Replay file whose first minute is garbage), shows where, and rescues the
//! good part without re-encoding it.
//!
//! - Decodes the video in 10 s chunks on the GPU (d3d11va: the video-decode
//!   unit, not the 3D engine; CPU fallback with 2 threads) and counts decode
//!   errors beyond the few every file prints when it's opened.
//! - Runs only while no game is in front, Hindsight isn't recording or saving,
//!   and stops between chunks the moment a game starts.
//! - Each clip is checked once: results are cached by size and date.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHUNK: f64 = 10.0;
/// Decode errors in a chunk beyond the "opening" ones before it counts as damaged.
const DAMAGED_OVER: usize = 20;
/// Bumped when the check changes; older "damaged" results are checked again.
/// 2: timestamp warnings from ffmpeg's output side no longer count as damage.
const VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub size: u64,
    pub modified: i64,
    /// "ok" | "damaged" | "unreadable"
    pub state: String,
    /// Damaged stretches, in seconds
    pub damaged: Vec<(f64, f64)>,
    /// The longest undamaged stretch (what a rescue keeps)
    pub best: Option<(f64, f64)>,
    pub duration: f64,
    /// Which version of the check made this result
    #[serde(default)]
    pub version: u32,
}

/// Is this ffmpeg error line about decoding the picture? Warnings from the
/// output side (e.g. "non monotonically increasing dts to muxer", printed for
/// every frame of a file with uneven timestamps) say nothing about damage.
pub fn is_decode_error(line: &str) -> bool {
    let l = line.trim();
    !l.is_empty() && !l.contains("Last message repeated") && !l.starts_with("[null @") && !l.contains("to muxer")
}

fn stamp(p: &Path) -> Option<(u64, i64)> {
    let m = fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((m.len(), t))
}

fn decode_errors(path: &str, ss: f64, t: f64, gpu: bool) -> Result<usize, String> {
    let mut a = crate::ffmpeg::args(&["-hide_banner", "-v", "error"]);
    if gpu {
        a.extend(crate::ffmpeg::args(&["-hwaccel", "d3d11va"]));
    } else {
        a.extend(crate::ffmpeg::args(&["-threads", "2"]));
    }
    a.extend(crate::ffmpeg::args(&["-ss", &format!("{ss:.3}"), "-t", &format!("{t:.3}"), "-i"]));
    a.push(path.into());
    a.extend(crate::ffmpeg::args(&["-map", "0:v:0", "-f", "null", "-"]));
    let out = crate::ffmpeg::cmd("ffmpeg").args(&a).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stderr);
    if gpu && (text.contains("Failed setup") || text.contains("hwaccel initialisation returned error")) {
        return Err("gpu".into());
    }
    Ok(text.lines().filter(|l| is_decode_error(l)).count())
}

/// Turn per-chunk error counts into damaged stretches and the best clean stretch.
pub fn summarize(chunk_errors: &[(f64, f64, usize)], baseline: usize, duration: f64) -> (Vec<(f64, f64)>, Option<(f64, f64)>) {
    let mut damaged: Vec<(f64, f64)> = Vec::new();
    for &(a, b, n) in chunk_errors {
        if n > baseline + DAMAGED_OVER {
            match damaged.last_mut() {
                Some(last) if (last.1 - a).abs() < 0.01 => last.1 = b,
                _ => damaged.push((a, b)),
            }
        }
    }
    // Longest gap between damaged stretches.
    let mut best: Option<(f64, f64)> = None;
    let mut start = 0.0;
    for &(a, b) in damaged.iter().chain(std::iter::once(&(duration, duration))) {
        if a - start > best.map(|(x, y)| y - x).unwrap_or(0.0) {
            best = Some((start, a));
        }
        start = b;
    }
    (damaged, best.filter(|(a, b)| b - a >= 1.0))
}

/// Check one clip. `keep_going` is asked between chunks (false = a game started: stop).
pub fn check(path: &str, keep_going: &dyn Fn() -> bool) -> Option<Health> {
    let (size, modified) = stamp(Path::new(path))?;
    let unreadable = |duration| Health { size, modified, state: "unreadable".into(), damaged: vec![], best: None, duration, version: VERSION };
    let p = match crate::library::probe(path) {
        Ok(p) if p.duration > 0.0 && !p.codec.is_empty() => p,
        _ => return Some(unreadable(0.0)),
    };
    let mut gpu = true;
    let mut run = |ss: f64, t: f64| -> Option<usize> {
        match decode_errors(path, ss, t, gpu) {
            Ok(n) => Some(n),
            Err(e) if e == "gpu" => {
                gpu = false;
                decode_errors(path, ss, t, false).ok()
            }
            Err(_) => None,
        }
    };
    // What every chunk prints just from opening the file.
    let baseline = run(0.0, 0.05)?;
    let mut chunks = Vec::new();
    let mut s = 0.0;
    while s < p.duration {
        if !keep_going() {
            return None;
        }
        let t = CHUNK.min(p.duration - s);
        chunks.push((s, s + t, run(s, t)?));
        s += CHUNK;
    }
    let (damaged, best) = summarize(&chunks, baseline, p.duration);
    Some(Health {
        size,
        modified,
        state: if damaged.is_empty() { "ok" } else { "damaged" }.into(),
        damaged,
        best,
        duration: p.duration,
        version: VERSION,
    })
}

/// Keep `from`–`to` of a damaged clip, copied untouched: starts at the first
/// keyframe at or after `from`. Saved next to it as "<name> (rescued)".
pub fn rescue(path: &str, from: f64, to: f64) -> Result<String, String> {
    let src = Path::new(path);
    let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "mp4".into());
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = src.parent().unwrap_or(Path::new("."));
    let mut out = dir.join(format!("{stem} (rescued).{ext}"));
    let mut n = 2;
    while out.exists() {
        out = dir.join(format!("{stem} (rescued {n}).{ext}"));
        n += 1;
    }
    // First keyframe at or after `from`.
    let mut a = crate::ffmpeg::args(&["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-read_intervals", &format!("{from:.3}%{:.3}", (from + 30.0).min(to)), "-show_entries", "frame=pts_time", "-of", "csv=p=0", "-i"]);
    a.push(path.into());
    let key = crate::ffmpeg::run("ffprobe", &a)?
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok())
        .find(|t| *t >= from - 0.01)
        .ok_or("No clean starting point (keyframe) found in the good part.")?;
    let mut a = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{key:.3}"), "-to", &format!("{to:.3}"), "-i"]);
    a.push(path.into());
    a.extend(crate::ffmpeg::args(&["-map", "0", "-c", "copy", "-avoid_negative_ts", "make_zero"]));
    if ext == "mp4" {
        a.extend(crate::ffmpeg::args(&["-movflags", "+faststart"]));
    }
    a.push(out.clone().into());
    crate::ffmpeg::run("ffmpeg", &a)?;
    Ok(out.to_string_lossy().into_owned())
}

/// Try to recover a damaged clip at full length: decode it while telling the
/// decoder to ignore errors and fill in broken pictures from the frames around
/// them (motion-guessed concealment), then encode that again on the GPU.
/// The damaged stretch usually comes out smeared rather than garbled, and
/// everything else is kept. Saved next to it as "<name> (repaired)"; the
/// original is never changed, so you can compare and choose.
pub fn repair(path: &str) -> Result<String, String> {
    let src = Path::new(path);
    let ext = match src.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("mkv") => "mkv",
        _ => "mp4",
    };
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = src.parent().unwrap_or(Path::new("."));
    let mut out = dir.join(format!("{stem} (repaired).{ext}"));
    let mut n = 2;
    while out.exists() {
        out = dir.join(format!("{stem} (repaired {n}).{ext}"));
        n += 1;
    }
    let info = crate::library::probe(path).map_err(|_| "This file can't be read at all, so there's nothing to repair.".to_string())?;
    if info.codec.is_empty() {
        return Err("This file has no picture to repair.".into());
    }
    let enc = match info.codec.as_str() {
        "hevc" => "hevc_nvenc",
        "av1" => "av1_nvenc",
        _ => "h264_nvenc",
    };
    let gop = (info.fps.round() as i64).max(1).to_string();
    let mut a = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-err_detect", "ignore_err", "-ec", "guess_mvs+deblock", "-fflags", "+genpts", "-i"]);
    a.push(path.into());
    a.extend(crate::ffmpeg::args(&[
        "-map", "0:v:0", "-map", "0:a?", "-c:v", enc, "-preset", "p5", "-cq", "20", "-b:v", "0", "-g", &gop, "-c:a", "aac", "-b:a", "192k",
    ]));
    if ext == "mp4" {
        a.extend(crate::ffmpeg::args(&["-movflags", "+faststart"]));
    }
    a.push(out.clone().into());
    // ffmpeg reports every broken frame as an error but still writes the file: judge by the result.
    let _ = crate::ffmpeg::run("ffmpeg", &a);
    let got = crate::library::probe(&out.to_string_lossy()).ok().filter(|p| !p.codec.is_empty() && p.duration > 0.5);
    match got {
        Some(p) if p.duration >= info.duration * 0.8 => {
            if let Ok(t) = fs::metadata(path).and_then(|m| m.modified())
                && let Ok(f) = fs::File::options().write(true).open(&out)
            {
                let _ = f.set_modified(t);
            }
            Ok(out.to_string_lossy().into_owned())
        }
        _ => {
            let _ = fs::remove_file(&out);
            Err("The repair didn't produce a usable video. Keeping the good part (Heal) is the way to go for this one.".into())
        }
    }
}

/// Network shares and mapped network drives have no Recycle Bin: anything
/// "recycled" there is gone for good.
pub fn on_network(path: &str) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::core::HSTRING;
    if path.starts_with(r"\\") || path.starts_with("//") {
        return true;
    }
    let Some(root) = path.get(..3).filter(|r| r.ends_with(":\\") || r.ends_with(":/")) else { return false };
    const DRIVE_REMOTE: u32 = 4;
    unsafe { GetDriveTypeW(&HSTRING::from(root)) == DRIVE_REMOTE }
}

/// Heal a damaged clip in place: keep only its longest good part (copied
/// untouched, from the first keyframe after the damage), check the result,
/// send the original to the Recycle Bin and give the healed file its name and
/// date, so it stays in the same spot in the library.
pub fn heal(path: &str, best: (f64, f64)) -> Result<String, String> {
    if on_network(path) {
        return Err("This clip is on a network folder (like the NAS), which has no Recycle Bin, so healing it in place could \
                    lose the original for good. Use \"Save the good part as a copy\" instead."
            .into());
    }
    let (a, b) = best;
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    let out = rescue(path, a, b)?;
    let healed = crate::library::replace_original(path, &out, Some(b - a)).inspect_err(|_| {
        let _ = fs::remove_file(&out);
    })?;
    if let Some(t) = modified
        && let Ok(f) = fs::File::options().write(true).open(&healed)
    {
        let _ = f.set_modified(t);
    }
    Ok(healed)
}

// ---------- results cache + background checker ----------

static CACHE: Mutex<Option<(PathBuf, BTreeMap<String, Health>)>> = Mutex::new(None);

fn with_cache<R>(file: &Path, f: impl FnOnce(&mut BTreeMap<String, Health>) -> R) -> R {
    let mut g = CACHE.lock().unwrap();
    if g.as_ref().map(|(p, _)| p != file).unwrap_or(true) {
        let map = fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        *g = Some((file.to_path_buf(), map));
    }
    f(&mut g.as_mut().unwrap().1)
}

fn save_cache(file: &Path) {
    let g = CACHE.lock().unwrap();
    if let Some((_, map)) = g.as_ref()
        && let Ok(t) = serde_json::to_string(map)
    {
        let tmp = file.with_extension("json.tmp");
        if fs::write(&tmp, t).is_ok() {
            let _ = fs::rename(&tmp, file);
        }
    }
}

/// Results for clips that are still the same file (size and date unchanged).
pub fn results(file: &Path, paths: &[String]) -> BTreeMap<String, Health> {
    // Look the results up under the lock, but read the files' sizes and dates after:
    // for clips on the NAS each one asks the network, and the background check needs the lock too.
    let found: Vec<(String, Health)> = with_cache(file, |m| paths.iter().filter_map(|p| Some((p.clone(), m.get(p)?.clone()))).collect());
    found
        .into_iter()
        .filter(|(p, h)| {
            // A "damaged" from an older check is checked again (it may have been a false alarm).
            let current = h.state == "ok" || h.version >= VERSION;
            current && stamp(Path::new(p)).is_some_and(|(size, modified)| h.size == size && h.modified == modified)
        })
        .collect()
}

pub fn forget(file: &Path, path: &str) {
    with_cache(file, |m| m.remove(path));
    save_cache(file);
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub checked: usize,
    pub total: usize,
    pub damaged: usize,
    /// Waiting because a game is in front, or a recording is running
    pub paused: bool,
    pub running: bool,
}

static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
pub fn progress() -> Progress {
    PROGRESS.lock().unwrap().clone().unwrap_or_default()
}

/// Background checker: clips that haven't been checked yet, one at a time,
/// only while the PC is free. `busy` says "a game is in front / recording".
pub fn start(
    cache_file: PathBuf,
    clips: impl Fn() -> Vec<String> + Send + 'static,
    enabled: impl Fn() -> bool + Send + 'static,
    busy: impl Fn() -> bool + Send + Sync + 'static,
    changed: impl Fn() + Send + 'static,
) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(45)); // let startup settle first
        loop {
            if !enabled() {
                *PROGRESS.lock().unwrap() = None;
                std::thread::sleep(Duration::from_secs(30));
                continue;
            }
            let all = clips();
            let done = results(&cache_file, &all);
            let damaged = done.values().filter(|h| h.state != "ok").count();
            let todo: Vec<&String> = all.iter().filter(|p| !done.contains_key(*p) && !p.starts_with(r"\\")).collect();
            let paused = busy();
            *PROGRESS.lock().unwrap() = Some(Progress { checked: all.len() - todo.len(), total: all.len(), damaged, paused, running: !todo.is_empty() });
            let Some(next) = todo.first().map(|s| s.to_string()) else {
                std::thread::sleep(Duration::from_secs(60));
                continue;
            };
            if paused {
                std::thread::sleep(Duration::from_secs(20));
                continue;
            }
            // Skip files still being written.
            let fresh = fs::metadata(&next)
                .and_then(|m| m.modified())
                .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(30))
                .unwrap_or(true);
            if fresh {
                std::thread::sleep(Duration::from_secs(20));
                continue;
            }
            if let Some(h) = check(&next, &|| !busy()) {
                let bad = h.state != "ok";
                with_cache(&cache_file, |m| m.insert(next.clone(), h));
                save_cache(&cache_file);
                if bad {
                    changed();
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long the health lookup takes for the whole real library (it used to run on
    /// the app's main thread). REWIND_NAS_DIR and REWIND_CLIPS_DIR, from a normal terminal.
    #[test]
    #[ignore]
    fn live_health_lookup_time() {
        let file = std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.rewind.app").join("health.json");
        let nas = std::env::var("REWIND_NAS_DIR").unwrap();
        let clips = std::env::var("REWIND_CLIPS_DIR").unwrap();
        let mut paths = Vec::new();
        for root in [nas, clips, r"D:\Medal\Clips".to_string(), r"D:\Recordings".to_string()] {
            let mut stack = vec![std::path::PathBuf::from(root)];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() { if !e.file_name().to_string_lossy().starts_with('.') { stack.push(p) } } else if crate::library::is_video(&p) { paths.push(p.to_string_lossy().into_owned()) }
                }
            }
        }
        let t = std::time::Instant::now();
        let r = results(&file, &paths);
        println!("{} clips, {} with results, lookup took {:?}", paths.len(), r.len(), t.elapsed());
    }

    #[test]
    fn damaged_chunks_merge_and_the_longest_clean_part_wins() {
        // 0-70 s damaged, rest fine (like the NVIDIA clip).
        let mut chunks: Vec<(f64, f64, usize)> = (0..18).map(|i| (i as f64 * 10.0, i as f64 * 10.0 + 10.0, if i < 7 { 1500 } else { 46 })).collect();
        chunks.last_mut().unwrap().1 = 180.2;
        let (d, best) = summarize(&chunks, 46, 180.2);
        assert_eq!(d, vec![(0.0, 70.0)]);
        assert_eq!(best, Some((70.0, 180.2)));
        // Clean file: nothing damaged, the whole thing is "best".
        let clean: Vec<_> = (0..3).map(|i| (i as f64 * 10.0, i as f64 * 10.0 + 10.0, 46)).collect();
        assert_eq!(summarize(&clean, 46, 30.0), (vec![], Some((0.0, 30.0))));
        // Damage in the middle: the longer side is kept.
        let mid = vec![(0.0, 10.0, 0), (10.0, 20.0, 999), (20.0, 50.0, 0)];
        assert_eq!(summarize(&mid, 0, 50.0).1, Some((20.0, 50.0)));
    }

    #[test]
    fn clips_on_the_network_are_never_healed_in_place() {
        let nas = format!("{0}{0}nas{0}clips{0}VALORANT{0}a.mp4", '\\');
        assert!(on_network(&nas));
        assert!(!on_network(r"C:\Users\me\Videos\a.mp4"));
        let err = heal(&nas, (70.0, 180.0)).unwrap_err();
        assert!(err.contains("no Recycle Bin"), "{err}");
    }

    /// A clip with garbage written over part of it: repair keeps its full length
    /// and the result decodes with far fewer errors.
    #[test]
    fn repair_keeps_the_full_length_of_a_damaged_clip() {
        crate::ffmpeg::init(None);
        let d = std::env::temp_dir().join(format!("rewind-repair-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let clip = d.join("a.mp4");
        let mut a = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-t", "8", "-c:v", "h264_nvenc", "-g", "60"]);
        a.push(clip.clone().into());
        crate::ffmpeg::run("ffmpeg", &a).unwrap();
        // Scribble over 30 KB in the middle (as a driver crash would).
        let mut bytes = fs::read(&clip).unwrap();
        let mid = bytes.len() / 2;
        for (i, b) in bytes[mid..mid + 30_000].iter_mut().enumerate() {
            *b = (i * 7919 % 251) as u8;
        }
        fs::write(&clip, &bytes).unwrap();
        let errors = |p: &str| decode_errors(p, 0.0, 8.0, false).unwrap();
        let before = errors(&clip.to_string_lossy());
        let out = repair(&clip.to_string_lossy()).unwrap();
        assert!(out.ends_with("a (repaired).mp4"));
        let p = crate::library::probe(&out).unwrap();
        assert!((p.duration - 8.0).abs() < 0.6, "{}", p.duration);
        let after = errors(&out);
        println!("decode errors before {before}, after {after}");
        assert!(before > 0 && after < before, "before {before}, after {after}");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn muxer_timestamp_warnings_are_not_damage() {
        assert!(!is_decode_error("[null @ 000002123df851c0] Application provided invalid, non monotonically increasing dts to muxer in stream 0: 99 >= 99"));
        assert!(!is_decode_error("    Last message repeated 3 times"));
        assert!(is_decode_error("[h264 @ 000001b959d4f040] no frame!"));
        assert!(is_decode_error("[vist#0:0/h264 @ 01] [dec:h264 @ 02] Error submitting packet to decoder: Invalid data found when processing input"));
    }

    /// Checks (and rescues) a real clip: REWIND_HEALTH_CLIP=path
    /// cargo test live_health -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_health() {
        crate::ffmpeg::init(None);
        let Ok(p) = std::env::var("REWIND_HEALTH_CLIP") else { return };
        let t = std::time::Instant::now();
        let h = check(&p, &|| true).unwrap();
        println!("{} in {:?}: damaged {:?}, best {:?} of {:.1} s", h.state, t.elapsed(), h.damaged, h.best, h.duration);
        if std::env::var("REWIND_HEALTH_RESCUE").is_ok()
            && let Some((a, b)) = h.best
            && h.state == "damaged"
        {
            let out = rescue(&p, a, b).unwrap();
            let again = check(&out, &|| true).unwrap();
            println!("rescued -> {out}: {} ({:.1} s, damaged {:?})", again.state, again.duration, again.damaged);
        }
    }
}

#[cfg(test)]
mod live_heal {
    /// Heals a COPY of a damaged clip and checks it again: REWIND_HEAL_CLIP=path
    /// cargo test live_heal -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_heal() {
        crate::ffmpeg::init(None);
        let Ok(p) = std::env::var("REWIND_HEAL_CLIP") else { return };
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        let h = super::check(&p, &|| true).unwrap();
        println!("before: {} {:?} best {:?} of {:.1}", h.state, h.damaged, h.best, h.duration);
        let t = std::time::Instant::now();
        let out = super::heal(&p, h.best.unwrap()).unwrap();
        println!("healed in {:?} -> {out}", t.elapsed());
        assert_eq!(out, p, "keeps the name");
        assert_eq!(std::fs::metadata(&out).unwrap().modified().unwrap(), before, "keeps the date");
        let again = super::check(&out, &|| true).unwrap();
        println!("after: {} {:?} of {:.1}", again.state, again.damaged, again.duration);
        assert_eq!(again.state, "ok");
    }
}
