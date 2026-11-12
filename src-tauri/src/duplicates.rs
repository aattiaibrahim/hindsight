//! The same clip in two places: another recorder's folder and Hindsight's
//! (after a copy instead of a move), Medal's Clips and Imported, Ascent's
//! Clips and Downloads, a copy someone made by hand.
//!
//! Files are only compared when they're exactly the same size, then by a
//! fingerprint of their first and last megabyte, so a big library is checked
//! in moments. Only files that match on both are read in full to confirm, so
//! two different clips are never called copies. Nothing is deleted here: the
//! app shows each set and you choose which copy goes.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const EDGE: u64 = 1 << 20;

/// Results so far, by (path, size, modified): a clip is only read once.
static SEEN: std::sync::Mutex<Option<HashMap<(String, u64, i64, bool), [u8; 32]>>> = std::sync::Mutex::new(None);

fn modified(path: &str) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn remembered(path: &str, size: u64, full: bool, f: impl FnOnce() -> Option<[u8; 32]>) -> Option<[u8; 32]> {
    let key = (path.to_string(), size, modified(path), full);
    if let Some(h) = SEEN.lock().unwrap().as_ref().and_then(|m| m.get(&key)) {
        return Some(*h);
    }
    let h = f()?;
    SEEN.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, h);
    Some(h)
}

fn full_hash(path: &str) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            return Some(h.finalize().into());
        }
        h.update(&buf[..n]);
    }
}

fn fingerprint(path: &str, size: u64) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; EDGE.min(size) as usize];
    f.read_exact(&mut buf).ok()?;
    h.update(&buf);
    if size > EDGE {
        f.seek(SeekFrom::Start(size.saturating_sub(EDGE).max(EDGE.min(size)))).ok()?;
        let mut tail = Vec::new();
        f.take(EDGE).read_to_end(&mut tail).ok()?;
        h.update(&tail);
    }
    h.update(size.to_le_bytes());
    Some(h.finalize().into())
}

/// Sets of paths holding the same clip (each set has 2+ paths). `clips` is
/// (path, size). Network paths are compared too, but read only when another
/// clip has their exact size.
pub fn find(clips: &[(String, u64)]) -> Vec<Vec<String>> {
    find_with(clips, &mut |_, _| {})
}

/// `find`, telling `progress` (files checked, files to check) as it goes.
pub fn find_with(clips: &[(String, u64)], progress: &mut dyn FnMut(usize, usize)) -> Vec<Vec<String>> {
    let mut by_size: HashMap<u64, Vec<&str>> = HashMap::new();
    for (p, s) in clips {
        if *s > 0 {
            by_size.entry(*s).or_default().push(p);
        }
    }
    // Only files that share their size with another are read at all.
    let total: usize = by_size.values().filter(|v| v.len() > 1).map(|v| v.len()).sum();
    let mut done = 0;
    progress(0, total);
    let mut out = Vec::new();
    for (size, paths) in by_size {
        if paths.len() < 2 {
            continue;
        }
        let mut by_print: HashMap<[u8; 32], Vec<String>> = HashMap::new();
        for p in paths {
            if let Some(fp) = remembered(p, size, false, || fingerprint(p, size)) {
                by_print.entry(fp).or_default().push(p.to_string());
            }
            done += 1;
            progress(done, total);
        }
        // Same edges: confirm by reading them whole.
        for candidates in by_print.into_values().filter(|v| v.len() > 1) {
            let mut by_full: HashMap<[u8; 32], Vec<String>> = HashMap::new();
            for p in candidates {
                if let Some(h) = remembered(&p, size, true, || full_hash(&p)) {
                    by_full.entry(h).or_default().push(p);
                }
            }
            out.extend(by_full.into_values().filter(|v| v.len() > 1));
        }
    }
    for set in &mut out {
        set.sort();
    }
    out.sort();
    out
}

/// The same recording saved in two formats: OBS's "Automatically remux to mp4"
/// (and Hindsight's own Change to .mp4) keep the original, so `2026-03-05 19-36-49.mkv`
/// sits next to `2026-03-05 19-36-49.mp4`. They differ byte for byte, so `find`
/// misses them. Here: same folder, same name, a different extension, and
/// `same` (length, picture, tracks) agrees. Sets of 2+ paths, sorted.
pub fn remuxes(clips: &[(String, u64)], same: impl Fn(&str, &str) -> bool) -> Vec<Vec<String>> {
    use std::path::Path;
    let mut by_name: HashMap<(String, String), Vec<&str>> = HashMap::new();
    for (p, _) in clips {
        let path = Path::new(p);
        let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else { continue };
        by_name.entry((dir.to_string_lossy().to_lowercase(), stem.to_string_lossy().to_lowercase())).or_default().push(p);
    }
    let ext = |p: &str| Path::new(p).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = Vec::new();
    for (_, paths) in by_name {
        if paths.len() < 2 {
            continue;
        }
        // Each file joins the first one it matches; a lookalike with the same name stays out.
        let mut sets: Vec<Vec<String>> = Vec::new();
        for p in paths {
            match sets.iter_mut().find(|s| s.iter().all(|q| ext(q) != ext(p)) && same(&s[0], p)) {
                Some(s) => s.push(p.to_string()),
                None => sets.push(vec![p.to_string()]),
            }
        }
        out.extend(sets.into_iter().filter(|s| s.len() > 1));
    }
    for set in &mut out {
        set.sort();
    }
    out.sort();
    out
}

/// Is one file a remux of the other: the same streams copied into another container?
/// Then they're nearly the same size (only the container differs) and the same
/// length, picture and tracks. A re-encode of the same recording (an export at a
/// third of the size) isn't: removing its original would lose quality.
pub fn same_streams((x, xs): (&crate::library::Probe, u64), (y, ys): (&crate::library::Probe, u64)) -> bool {
    let near = xs > 0 && ys > 0 && (xs.max(ys) - xs.min(ys)) as f64 <= xs.max(ys) as f64 * 0.05;
    near && x.codec == y.codec && same_recording(x, y)
}

/// The same recording, however it was saved: same length, picture and number of
/// tracks. With `same_streams` false, one is a re-encode (an export, a compressed
/// copy) of the other: smaller, and not as good.
pub fn same_recording(x: &crate::library::Probe, y: &crate::library::Probe) -> bool {
    x.duration > 0.0
        && (x.duration - y.duration).abs() <= 0.5_f64.max(x.duration * 0.005)
        && (x.width, x.height) == (y.width, y.height)
        && x.audio_tracks == y.audio_tracks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remux_has_the_same_streams_but_a_re_encode_does_not() {
        let p = |duration: f64, codec: &str| crate::library::Probe { duration, codec: codec.into(), width: 1920, height: 1080, fps: 60.0, audio_tracks: 2, container: String::new() };
        let mkv = p(772.9, "h264");
        // OBS's remux: a little smaller (MP4's container), same everything else.
        assert!(same_streams((&mkv, 2_386 << 20), (&p(772.9, "h264"), 2_370 << 20)));
        // An export of the same recording: same length and codec, a third of the size.
        assert!(!same_streams((&mkv, 2_386 << 20), (&p(772.9, "h264"), 750 << 20)));
        assert!(!same_streams((&mkv, 100), (&p(772.9, "hevc"), 100)), "another codec");
        assert!(!same_streams((&mkv, 100), (&p(700.0, "h264"), 100)), "another length");
        assert!(!same_streams((&p(0.0, "h264"), 100), (&p(0.0, "h264"), 100)), "unreadable");
        // ...but the export is still the same recording, in a smaller copy.
        assert!(same_recording(&mkv, &p(772.9, "h264")));
        assert!(same_recording(&mkv, &p(772.9, "hevc")));
        assert!(!same_recording(&mkv, &p(700.0, "h264")));
    }


    /// Real files: an MKV, its MP4 remux (streams copied, like OBS) and a smaller
    /// re-encode with the same name in another folder's worth of formats.
    #[test]
    fn a_real_remux_is_found_and_a_re_encode_is_not() {
        use crate::ffmpeg;
        ffmpeg::init(None);
        let d = std::env::temp_dir().join(format!("rewind-remux-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let run = |a: &[&str], out: &std::path::Path| {
            let mut v = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y"]);
            v.extend(ffmpeg::args(a));
            v.push(out.into());
            ffmpeg::run("ffmpeg", &v).unwrap();
        };
        let mkv = d.join("2026-03-05 19-36-49.mkv");
        run(&["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "4", "-c:v", "mpeg4", "-q:v", "2", "-c:a", "aac"], &mkv);
        let mkv_s = mkv.to_string_lossy().into_owned();
        run(&["-i", &mkv_s, "-map", "0", "-c", "copy"], &d.join("2026-03-05 19-36-49.mp4"));
        let other = d.join("Export");
        std::fs::create_dir_all(&other).unwrap();
        let export_src = other.join("2026-03-05 19-36-49.mkv");
        std::fs::copy(&mkv, &export_src).unwrap();
        run(&["-i", &export_src.to_string_lossy(), "-c:v", "mpeg4", "-q:v", "20", "-c:a", "aac"], &other.join("2026-03-05 19-36-49.mp4"));
        let list: Vec<(String, u64)> = [&mkv, &d.join("2026-03-05 19-36-49.mp4"), &export_src, &other.join("2026-03-05 19-36-49.mp4")]
            .iter()
            .map(|p| (p.to_string_lossy().into_owned(), std::fs::metadata(p).unwrap().len()))
            .collect();
        let size = |p: &str| list.iter().find(|(q, _)| q == p).map(|(_, s)| *s).unwrap_or(0);
        let sets = remuxes(&list, |a, b| match (crate::library::probe(a), crate::library::probe(b)) {
            (Ok(x), Ok(y)) => same_streams((&x, size(a)), (&y, size(b))),
            _ => false,
        });
        assert_eq!(sets.len(), 1, "only the remux: {sets:?}");
        assert!(!sets[0][0].contains("Export"));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn remuxes_are_the_same_name_in_another_format_and_the_same_recording() {
        let list: Vec<(String, u64)> = [
            r"C:\OBS\2026-03-05 19-36-49.mkv",
            r"C:\OBS\2026-03-05 19-36-49.mp4",
            r"C:\OBS\2026-03-05 19-40-00.mkv",     // no partner
            r"C:\OBS\Other\2026-03-05 19-36-49.mp4", // same name, other folder
            r"C:\OBS\clip.mkv",
            r"C:\OBS\clip.mp4",                    // same name, but a different recording
        ]
        .iter()
        .map(|p| (p.to_string(), 1))
        .collect();
        let sets = remuxes(&list, |a, b| !(a.contains("clip") && b.contains("clip")));
        assert_eq!(sets.len(), 1, "{sets:?}");
        assert!(sets[0][0].ends_with("19-36-49.mkv") && sets[0][1].ends_with("19-36-49.mp4") && !sets[0][1].contains("Other"));
    }

    #[test]
    fn finds_copies_not_lookalikes() {
        let d = std::env::temp_dir().join(format!("rewind-dupes-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let clip: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut other = clip.clone();
        other[1_500_000] ^= 1; // same size, same start and end... different middle
        let mut edited = clip.clone();
        edited[10] ^= 1; // same size, different start
        for (n, b) in [("a.mp4", &clip), ("b.mp4", &clip), ("c.mp4", &edited), ("tiny1.mp4", &vec![1u8; 10]), ("tiny2.mp4", &vec![1u8; 10])] {
            std::fs::write(d.join(n), b).unwrap();
        }
        std::fs::write(d.join("lookalike.mp4"), &other).unwrap();
        let list: Vec<(String, u64)> = ["a.mp4", "b.mp4", "c.mp4", "lookalike.mp4", "tiny1.mp4", "tiny2.mp4"]
            .iter()
            .map(|n| (d.join(n).to_string_lossy().into_owned(), std::fs::metadata(d.join(n)).unwrap().len()))
            .collect();
        let sets = find(&list);
        assert_eq!(sets.len(), 2, "{sets:?}");
        assert!(sets.iter().any(|s| s.len() == 2 && s[0].ends_with("a.mp4") && s[1].ends_with("b.mp4")));
        assert!(sets.iter().any(|s| s[0].ends_with("tiny1.mp4")));
        let _ = std::fs::remove_dir_all(d);
    }
}

#[cfg(test)]
mod live {

    /// What Cleanup's duplicate search finds in the real library, the way the app does
    /// it (saved details, NAS listing). REWIND_NAS_DIR, REWIND_CLIPS_DIR; normal terminal.
    #[test]
    #[ignore]
    fn live_cleanup_remuxes() {
        crate::ffmpeg::init(None);
        let nas = std::env::var("REWIND_NAS_DIR").unwrap();
        let clips = std::env::var("REWIND_CLIPS_DIR").unwrap();
        crate::prepare::init(std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.rewind.app").join("probes.json"));
        crate::library::set_twin(&nas, &clips);
        let listing = crate::nas::scan_nas(std::path::Path::new(&nas));
        let all = crate::library::scan_all(std::path::Path::new(&clips), &[], &listing, &[], &nas);
        let named: Vec<(String, u64)> = all.iter().map(|c| (c.path.clone(), c.size)).collect();
        let obs: Vec<&(String, u64)> = named.iter().filter(|(p, _)| p.contains("03-14-17")).collect();
        println!("{} clips ({} on the NAS only); the OBS pair in the list: {obs:?}", all.len(), all.iter().filter(|c| c.location == "nas").count());
        for (p, _) in &obs {
            println!("  {p}: {:?}", crate::prepare::probe(p).map(|x| (x.duration, x.width, x.height, x.codec, x.audio_tracks)));
        }
        let sizes: std::collections::HashMap<&str, u64> = named.iter().map(|(p, s)| (p.as_str(), *s)).collect();
        let size = |p: &str| sizes.get(p).copied().unwrap_or(0);
        let sets = super::remuxes(&named, |a, b| matches!((crate::prepare::probe(a), crate::prepare::probe(b)), (Ok(x), Ok(y)) if super::same_streams((&x, size(a)), (&y, size(b)))));
        println!("remux sets: {sets:?}");
    }

    /// Real folder: REWIND_REMUX_DIR=D:\Recordings cargo test live_remuxes -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_remuxes() {
        crate::ffmpeg::init(None);
        let Ok(dir) = std::env::var("REWIND_REMUX_DIR") else { return };
        let mut list = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(p) = stack.pop() {
            for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if crate::library::is_video(&path) {
                    list.push((path.to_string_lossy().into_owned(), e.metadata().map(|m| m.len()).unwrap_or(0)));
                }
            }
        }
        let size = |p: &str| list.iter().find(|(q, _)| q == p).map(|(_, s)| *s).unwrap_or(0);
        let probe = |p: &str| crate::library::probe(p);
        let remux = super::remuxes(&list, |a, b| matches!((probe(a), probe(b)), (Ok(x), Ok(y)) if super::same_streams((&x, size(a)), (&y, size(b)))));
        let smaller = super::remuxes(&list, |a, b| matches!((probe(a), probe(b)), (Ok(x), Ok(y)) if super::same_recording(&x, &y)));
        println!("{} clips; remux sets: {remux:?}; same recording: {smaller:?}", list.len());
    }
    /// Real folders: REWIND_DUPE_DIRS="dir1;dir2" cargo test live_duplicates -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_duplicates() {
        let Ok(dirs) = std::env::var("REWIND_DUPE_DIRS") else { return };
        let mut list = Vec::new();
        for d in dirs.split(';') {
            let mut stack = vec![std::path::PathBuf::from(d)];
            while let Some(p) = stack.pop() {
                for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if crate::library::is_video(&path) {
                        list.push((path.to_string_lossy().into_owned(), e.metadata().map(|m| m.len()).unwrap_or(0)));
                    }
                }
            }
        }
        let t = std::time::Instant::now();
        let sets = super::find(&list);
        println!("{} clips, {} duplicate sets in {:?}", list.len(), sets.len(), t.elapsed());
        let t = std::time::Instant::now();
        super::find(&list);
        println!("again: {:?}", t.elapsed());
        for s in sets.iter().take(15) {
            println!("  {}", s.join("  ==  "));
        }
    }
}
