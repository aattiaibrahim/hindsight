//! Smooth scrubbing for clips whose keyframes are far apart.
//!
//! A player can only start decoding at a keyframe, so to show any spot it decodes
//! from the keyframe before it. Hindsight records one a second, but other recorders
//! (Ascent, some OBS setups) can leave 5–10 s between them: every jump while
//! scrubbing then decodes hundreds of frames (about 7 frames a second in Chrome,
//! against about 58), which looks frozen or torn.
//!
//! For those clips Studio plays a preview copy instead, made once on the GPU with a
//! keyframe every half second and kept in the app's cache. Only the preview uses it:
//! trims, saves and exports still read the original.

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Keyframes further apart than this (seconds) make scrubbing slow enough to fix.
pub const SLOW_GAP: f64 = 2.5;
/// The cache keeps at most this much, and nothing older than this.
const KEEP_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const KEEP_AGE: Duration = Duration::from_secs(14 * 24 * 3600);

/// The largest gap between keyframes in the first 30 s (seconds), None if it can't tell.
/// A clip with a single keyframe in that stretch counts as the whole stretch.
pub fn keyframe_gap(path: &str) -> Option<f64> {
    let mut a = crate::ffmpeg::args(&["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-read_intervals", "%+30", "-show_entries", "frame=pts_time", "-of", "csv=p=0"]);
    a.push(path.into());
    let text = crate::ffmpeg::run("ffprobe", &a).ok()?;
    let keys: Vec<f64> = text.lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
    gap_of(&keys, crate::library::probe(path).ok().map(|p| p.duration))
}

fn gap_of(keys: &[f64], duration: Option<f64>) -> Option<f64> {
    match keys {
        [] => None,
        [_] => Some(duration.unwrap_or(30.0).min(30.0)),
        _ => keys.windows(2).map(|w| w[1] - w[0]).fold(None, |m: Option<f64>, g| Some(m.map_or(g, |m| m.max(g)))),
    }
}

/// Where a clip's copy goes: named after its path, size and date, so an edited
/// clip (same name, new content) gets a new copy.
fn copy_for(dir: &Path, path: &str) -> Option<PathBuf> {
    let m = fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis();
    let mut h = Sha256::new();
    h.update(path.to_lowercase().as_bytes());
    h.update(m.len().to_le_bytes());
    h.update(mtime.to_le_bytes());
    let name: String = h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect();
    Some(dir.join(format!("{name}.mp4")))
}

/// Clips being copied right now (opening the same clip twice waits for the first).
static MAKING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// The smooth copy to play for `path`: None when the clip scrubs fine as it is.
/// Makes it the first time (reporting seconds done to `progress`), else reuses it.
pub fn smooth_copy(dir: &Path, path: &str, progress: impl Fn(f64) + 'static) -> Result<Option<PathBuf>, String> {
    let out = copy_for(dir, path).ok_or("That clip can't be read.")?;
    if out.is_file() {
        let _ = filetime_touch(&out);
        return Ok(Some(out));
    }
    if keyframe_gap(path).is_none_or(|g| g <= SLOW_GAP) {
        return Ok(None);
    }
    // One at a time per clip.
    loop {
        let mut m = MAKING.lock().unwrap();
        let set = m.get_or_insert_with(HashSet::new);
        if set.insert(path.to_lowercase()) {
            break;
        }
        drop(m);
        std::thread::sleep(Duration::from_millis(400));
        if out.is_file() {
            return Ok(Some(out));
        }
    }
    let result = make(dir, path, &out, progress);
    MAKING.lock().unwrap().get_or_insert_with(HashSet::new).remove(&path.to_lowercase());
    result.map(Some)
}

fn make(dir: &Path, path: &str, out: &Path, progress: impl Fn(f64) + 'static) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    prune(dir);
    let tmp = out.with_extension("making.mp4");
    let _ = fs::remove_file(&tmp);
    // H.264 on the GPU, a keyframe every half second (at 60 fps) and no B-frames, so any
    // spot is at most 30 frames from one; at most 1080p (it's only a preview). Every
    // audio track comes along in order, so the preview sounds the same.
    let mut a = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-i"]);
    a.push(path.into());
    a.extend(crate::ffmpeg::args(&[
        "-map", "0:v:0", "-map", "0:a?",
        "-vf", "scale=-2:'min(1080,ih)'",
        "-c:v", "h264_nvenc", "-preset", "p2", "-cq", "27", "-g", "30", "-bf", "0", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "160k",
        "-movflags", "+faststart",
    ]));
    a.push(tmp.clone().into());
    let ran = crate::ffmpeg::with_progress(progress, || crate::ffmpeg::run("ffmpeg", &a));
    if let Err(e) = ran {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Couldn't make a smooth-scrubbing copy: {e}"));
    }
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(out.to_path_buf())
}

/// Mark a copy as just used (the cache drops the least recently used first).
fn filetime_touch(p: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new().append(true).open(p)?.set_modified(SystemTime::now())
}

/// Drop copies older than two weeks, then the least recently used beyond 8 GB.
fn prune(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    let now = SystemTime::now();
    files.retain(|(p, _, t)| {
        let old = now.duration_since(*t).unwrap_or_default() > KEEP_AGE;
        if old {
            let _ = fs::remove_file(p);
        }
        !old
    });
    files.sort_by(|a, b| b.2.cmp(&a.2));
    let mut total = 0u64;
    for (p, len, _) in files {
        total += len;
        if total > KEEP_BYTES {
            let _ = fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_biggest_gap_between_keyframes_is_what_counts() {
        assert_eq!(gap_of(&[0.0, 1.0, 2.0, 3.0], None), Some(1.0));
        assert_eq!(gap_of(&[0.0, 1.0, 9.5], None), Some(8.5));
        assert_eq!(gap_of(&[0.0], Some(12.0)), Some(12.0));
        assert_eq!(gap_of(&[], None), None);
    }

    /// Makes a clip with a keyframe every 10 s, then its smooth copy, and checks the
    /// copy has a keyframe at least every second, the same length and both tracks.
    #[test]
    #[ignore]
    fn live_smooth_copy() {
        crate::ffmpeg::init(None);
        let d = std::env::temp_dir().join(format!("hs-scrub-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let src = d.join("slow.mp4");
        let mut a = crate::ffmpeg::args(&[
            "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=60",
            "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono",
            "-t", "20", "-map", "0:v", "-map", "1:a", "-map", "2:a", "-c:v", "h264_nvenc", "-g", "600", "-bf", "2", "-c:a", "aac",
        ]);
        a.push(src.clone().into());
        crate::ffmpeg::run("ffmpeg", &a).unwrap();
        let s = src.to_string_lossy().into_owned();
        assert!(keyframe_gap(&s).unwrap() > SLOW_GAP);
        let cache = d.join("cache");
        let t = std::time::Instant::now();
        let out = smooth_copy(&cache, &s, |_| {}).unwrap().expect("a copy for a slow clip");
        println!("copy made in {:.1} s", t.elapsed().as_secs_f64());
        let o = out.to_string_lossy().into_owned();
        assert!(keyframe_gap(&o).unwrap() <= 1.0, "gap {:?}", keyframe_gap(&o));
        let (pi, po) = (crate::library::probe(&s).unwrap(), crate::library::probe(&o).unwrap());
        assert!((pi.duration - po.duration).abs() < 0.2, "{} vs {}", pi.duration, po.duration);
        assert_eq!(po.audio_tracks, 2);
        // The second time it's the same file, made once.
        assert_eq!(smooth_copy(&cache, &s, |_| {}).unwrap(), Some(out));
        let _ = fs::remove_dir_all(&d);
    }
}
