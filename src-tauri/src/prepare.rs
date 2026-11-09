//! Make the library open fast, even with clips on a slow hard drive.
//!
//! - Clip details (length, codec, size) are remembered on this PC's app data
//!   folder (usually the SSD), so they're read from the clip only once.
//! - A background job prepares every clip's thumbnail and details ahead of
//!   time, one clip at a time, only while no game is running, so scrolling
//!   the library never waits on the drive.

use crate::library::{self, Probe};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    size: u64,
    modified: i64,
    probe: Probe,
}

struct Cache {
    file: PathBuf,
    map: HashMap<String, Entry>,
    dirty: bool,
    saved: Instant,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn stamp(p: &Path) -> Option<(u64, i64)> {
    let m = fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((m.len(), t))
}

/// Where remembered details live. Set once at startup.
pub fn init(file: PathBuf) {
    let map = fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    *CACHE.lock().unwrap() = Some(Cache { file, map, dirty: false, saved: Instant::now() });
}

fn save(c: &mut Cache) {
    if let Ok(t) = serde_json::to_string(&c.map) {
        let tmp = c.file.with_extension("json.tmp");
        if fs::write(&tmp, t).is_ok() {
            let _ = fs::rename(&tmp, &c.file);
        }
    }
    c.dirty = false;
    c.saved = Instant::now();
}

/// `library::probe`, remembered until the file changes.
pub fn probe(path: &str) -> Result<Probe, String> {
    let st = stamp(Path::new(path));
    if let (Some((size, modified)), Some(c)) = (st, CACHE.lock().unwrap().as_ref())
        && let Some(e) = c.map.get(path)
        && e.size == size
        && e.modified == modified
    {
        return Ok(e.probe.clone());
    }
    // A clip on the NAS whose copy was on this PC: same size and date, same details.
    if let (Some((size, modified)), Some(twin)) = (st, library::local_twin(path))
        && let Some(c) = CACHE.lock().unwrap().as_mut()
        && let Some(e) = c.map.get(&twin).filter(|e| e.size == size && e.modified == modified).cloned()
    {
        c.map.insert(path.to_string(), e.clone());
        c.dirty = true;
        return Ok(e.probe);
    }
    let p = library::probe(path)?;
    if let (Some((size, modified)), Some(c)) = (st, CACHE.lock().unwrap().as_mut()) {
        c.map.insert(path.to_string(), Entry { size, modified, probe: p.clone() });
        c.dirty = true;
        // Saved in batches, not after every clip.
        if c.saved.elapsed() > Duration::from_secs(5) {
            save(c);
        }
    }
    Ok(p)
}

pub fn flush() {
    if let Some(c) = CACHE.lock().unwrap().as_mut()
        && c.dirty
    {
        save(c);
    }
}

fn known(path: &str) -> bool {
    let st = stamp(Path::new(path));
    let twin = library::local_twin(path);
    CACHE.lock().unwrap().as_ref().is_some_and(|c| {
        [Some(path.to_string()), twin].into_iter().flatten().any(|k| c.map.get(&k).is_some_and(|e| Some((e.size, e.modified)) == st))
    })
}

/// How many of the newest clips get their audio (waveforms, extra tracks) ready
/// too, so they open in Studio without reading the file three times at once.
const AUDIO_AHEAD: usize = 30;

/// Background preparer: details + thumbnail for every clip not ready yet, then
/// the audio for the newest few.
pub fn start(
    thumbs: PathBuf,
    clips: impl Fn() -> Vec<String> + Send + 'static,
    enabled: impl Fn() -> bool + Send + 'static,
    busy: impl Fn() -> bool + Send + 'static,
    audio: impl Fn(&str) + Send + 'static,
) {
    std::thread::Builder::new()
        .name("rewind-prepare".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(20)); // let startup settle first
            let mut audio_done: std::collections::HashSet<(String, Option<(u64, i64)>)> = Default::default();
            loop {
                if !enabled() || busy() {
                    flush();
                    std::thread::sleep(Duration::from_secs(20));
                    continue;
                }
                // This PC's clips first, then the ones only on the NAS (slower to read,
                // so they're ready before you open Synced clips instead of while you look).
                let mut todo: Vec<String> = clips().into_iter().filter(|p| !known(p) || !library::thumbnail_exists(p, &thumbs)).collect();
                todo.sort_by_key(|p| p.starts_with(r"\\"));
                if todo.is_empty() {
                    flush();
                    // The newest clips' audio, the ones you're likely to open next.
                    let mut recent: Vec<(String, Option<(u64, i64)>)> = clips()
                        .into_iter()
                        .filter(|p| !p.starts_with(r"\\"))
                        .map(|p| {
                            let st = stamp(Path::new(&p));
                            (p, st)
                        })
                        .collect();
                    recent.sort_by_key(|(_, st)| std::cmp::Reverse(st.map(|s| s.1)));
                    for item in recent.into_iter().take(AUDIO_AHEAD) {
                        if !enabled() || busy() {
                            break;
                        }
                        if audio_done.contains(&item) {
                            continue;
                        }
                        audio(&item.0);
                        audio_done.insert(item);
                        std::thread::sleep(Duration::from_millis(150));
                    }
                    // Everything's ready: look for duplicates too, so Cleanup opens instantly.
                    let sized: Vec<(String, u64)> = clips().into_iter().filter(|p| !p.starts_with(r"\\")).filter_map(|p| fs::metadata(&p).ok().map(|m| (p, m.len()))).collect();
                    let _ = crate::duplicates::find(&sized);
                    std::thread::sleep(Duration::from_secs(300));
                    continue;
                }
                for p in todo {
                    if !enabled() || busy() {
                        break;
                    }
                    let _ = probe(&p);
                    let _ = library::thumbnail(&p, &thumbs);
                    // Gentle on the drive: the library itself comes first.
                    std::thread::sleep(Duration::from_millis(150));
                }
                flush();
            }
        })
        .expect("prepare thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many NAS clips already have their details in the app's real cache, and how
    /// long a probe takes for one that doesn't. Run from a normal terminal with
    /// REWIND_NAS_DIR and REWIND_CLIPS_DIR set.
    #[test]
    #[ignore]
    fn live_nas_probe_count() {
        crate::ffmpeg::init(None);
        let nas = std::env::var("REWIND_NAS_DIR").unwrap();
        let clips = std::env::var("REWIND_CLIPS_DIR").unwrap();
        init(std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.rewind.app").join("probes.json"));
        crate::library::set_twin(&nas, &clips);
        let (mut have, mut missing) = (0, Vec::new());
        for g in std::fs::read_dir(&nas).unwrap().flatten().filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.')) {
            for f in std::fs::read_dir(g.path()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| crate::library::is_video(p)) {
                let p = f.to_string_lossy().into_owned();
                if known(&p) { have += 1 } else { missing.push(p) }
            }
        }
        println!("probes known {have}, missing {}", missing.len());
        for m in missing.iter().take(3) {
            let t = std::time::Instant::now();
            let r = crate::library::probe(m).map(|x| x.duration);
            println!("  {m}: {r:?} in {:?}", t.elapsed());
        }
    }

    #[test]
    fn details_are_read_once_and_again_when_the_file_changes() {
        crate::ffmpeg::init(None);
        let dir = std::env::temp_dir().join(format!("rewind-prepare-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("a.mkv");
        let mut args = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30", "-t", "2", "-c:v", "mpeg4"]);
        args.push(clip.clone().into());
        crate::ffmpeg::run("ffmpeg", &args).unwrap();
        init(dir.join("probes.json"));
        let path = clip.to_string_lossy().into_owned();
        let first = probe(&path).unwrap();
        assert!((first.duration - 2.0).abs() < 0.2);
        assert!(known(&path));
        flush();
        // A fresh start reads it back from the saved file.
        init(dir.join("probes.json"));
        assert!(known(&path));
        // The file changes: no longer known.
        fs::write(&clip, b"not a video").unwrap();
        assert!(!known(&path));
        let _ = fs::remove_dir_all(dir);
    }
}
