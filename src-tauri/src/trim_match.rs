//! Clips already trimmed into a separate file: another app (Medal's "Edits", say)
//! or "save as a new file" kept the full-length original next to the cut, so by its
//! length alone the original still looks untrimmed and sits on To trim forever.
//!
//! The app picks the candidates (shorter clips of the same game saved after the
//! original); here each is checked by its pictures: tiny grey frames of the cut
//! (8 a second) against the original's keyframes. They're the same clip only when
//! several keyframes match at one steady offset and cover most of the cut's span,
//! so two clips of the same menu or loading screen don't pass. Results are kept in
//! app data, so each original is read once (again only if new candidates turn up).

use std::collections::HashMap;
use std::path::Path;

const W: usize = 32;
const H: usize = 18;
const N: usize = W * H;
/// Mean grey difference (0-255) under which two tiny frames are the same picture. The
/// same frame re-encoded differs by ~0-2, a tenth of a second apart in fast motion by up
/// to ~12, two different moments of the same game by 20-90.
const SAME: f64 = 12.0;
/// The cut's frames a second.
const RATE: f64 = 8.0;
/// Frames flatter than this (black, a fade, a plain loading screen) say nothing.
const FLAT: f64 = 6.0;

#[derive(serde::Deserialize, Clone)]
pub struct Candidate {
    pub original: String,
    /// Shorter clips that may have been cut from it, likeliest first.
    pub edits: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct Entry {
    size: u64,
    modified: i64,
    /// Candidates already compared (lower case).
    checked: Vec<String>,
    /// The cut it was trimmed into.
    into: Option<String>,
}

type Prints = Vec<(f64, Vec<u8>)>;

fn stamp(path: &str) -> Option<(u64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((m.len(), t))
}

/// Tiny grey frames with their times: every keyframe (`keyframes`, quick: the rest
/// isn't decoded), or RATE a second. Decoded on the graphics card where it can
/// (several times faster: some recorders put a keyframe every 0.4 s), else on the CPU.
fn prints(path: &str, keyframes: bool) -> Option<Prints> {
    if cfg!(windows)
        && let Some(p) = prints_with(path, keyframes, true)
    {
        return Some(p);
    }
    prints_with(path, keyframes, false)
}

fn prints_with(path: &str, keyframes: bool, gpu: bool) -> Option<Prints> {
    let mut c = crate::ffmpeg::cmd("ffmpeg");
    c.args(["-hide_banner", "-nostats", "-loglevel", "info"]);
    if gpu {
        c.args(["-hwaccel", "d3d11va"]);
    }
    if keyframes {
        c.args(["-skip_frame", "nokey"]);
    }
    let vf = format!("{}scale={W}:{H}:flags=area,format=gray,showinfo", if keyframes { String::new() } else { format!("fps={RATE},") });
    c.arg("-i").arg(path).args(["-map", "0:v:0", "-an", "-sn", "-vf", &vf, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"]);
    let o = c.output().ok()?;
    let times: Vec<f64> = String::from_utf8_lossy(&o.stderr)
        .lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| l.split("pts_time:").nth(1)?.split_whitespace().next()?.parse().ok())
        .collect();
    let frames = o.stdout.chunks_exact(N);
    let out: Prints = times.into_iter().zip(frames).map(|(t, f)| (t, f.to_vec())).collect();
    (!out.is_empty()).then_some(out)
}

fn diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64).sum::<u64>() as f64 / N as f64
}

fn spread(a: &[u8]) -> f64 {
    let m = a.iter().map(|x| *x as f64).sum::<f64>() / a.len() as f64;
    (a.iter().map(|x| (*x as f64 - m).powi(2)).sum::<f64>() / a.len() as f64).sqrt()
}

/// Was `cut` (RATE frames a second) cut out of `original` (its keyframes)?
pub(crate) fn cut_from(original: &Prints, cut: &Prints) -> bool {
    let Some(len) = cut.last().map(|(t, _)| t - cut[0].0 + 1.0 / RATE) else { return false };
    // Each telling keyframe's best match in the cut, as an offset (where the cut starts in the original).
    let offsets: Vec<f64> = original
        .iter()
        .filter(|(_, f)| spread(f) >= FLAT)
        .filter_map(|(t, f)| {
            let (d, ct) = cut.iter().map(|(ct, cf)| (diff(f, cf), *ct)).min_by(|a, b| a.0.total_cmp(&b.0))?;
            (d < SAME).then_some(t - (ct - cut[0].0))
        })
        .collect();
    // The steadiest offset: the most matches within half a second of each other.
    let best = offsets
        .iter()
        .map(|o| (*o, offsets.iter().filter(|x| (*x - o).abs() <= 0.5).count()))
        .max_by_key(|(_, n)| *n);
    let Some((start, hits)) = best else { return false };
    // Telling keyframes the cut spans: a good share of them have to match there (fast
    // motion between a keyframe and the cut's nearest frame makes some miss).
    let spanned = original.iter().filter(|(t, f)| *t >= start - 0.3 && *t <= start + len + 0.3 && spread(f) >= FLAT).count();
    hits >= 3 && hits * 3 >= spanned
}

fn load(file: &Path) -> HashMap<String, Entry> {
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(file: &Path, m: &HashMap<String, Entry>) {
    if let Ok(t) = serde_json::to_string(m) {
        let tmp = file.with_extension("tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

/// Originals already known to be trimmed into another file: original → the cut.
pub fn known(file: &Path) -> HashMap<String, String> {
    load(file).into_iter().filter_map(|(k, e)| Some((k, e.into?))).collect()
}

/// Check each candidate (stopping early when `stop` says so). `progress` gets
/// (originals done, originals to check), `found` each match as it's made. Returns
/// every match among `list`, old and new.
pub fn check(
    file: &Path,
    list: &[Candidate],
    progress: &mut dyn FnMut(usize, usize),
    found: &mut dyn FnMut(&str, &str),
    stop: &dyn Fn() -> bool,
) -> HashMap<String, String> {
    let mut cache = load(file);
    let mut out = HashMap::new();
    let mut cuts: HashMap<String, Option<Prints>> = HashMap::new();
    // What still needs reading: an original that changed, or has candidates it wasn't compared with.
    let mut todo = Vec::new();
    for c in list {
        let Some((size, modified)) = stamp(&c.original) else { continue };
        let e = cache.get(&c.original).filter(|e| e.size == size && e.modified == modified);
        if let Some(into) = e.and_then(|e| e.into.clone()) {
            out.insert(c.original.clone(), into);
            continue;
        }
        let checked: Vec<String> = e.map(|e| e.checked.clone()).unwrap_or_default();
        let new: Vec<String> = c.edits.iter().filter(|p| !checked.contains(&p.to_lowercase())).cloned().collect();
        if !new.is_empty() {
            todo.push((c.original.clone(), size, modified, checked, new));
        }
    }
    let total = todo.len();
    progress(0, total);
    for (i, (original, size, modified, mut checked, new)) in todo.into_iter().enumerate() {
        if stop() {
            break;
        }
        let mut into = None;
        if let Some(orig) = prints(&original, true) {
            for e in &new {
                let cut = cuts.entry(e.clone()).or_insert_with(|| prints(e, false));
                let Some(cut) = cut else { continue }; // unreadable: tried again next time
                checked.push(e.to_lowercase());
                if cut_from(&orig, cut) {
                    into = Some(e.clone());
                    break;
                }
            }
        }
        if let Some(e) = &into {
            found(&original, e);
            out.insert(original.clone(), e.clone());
        }
        cache.insert(original, Entry { size, modified, checked, into });
        save(file, &cache);
        progress(i + 1, total);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with some detail, different for each `seed`.
    fn frame(seed: u32) -> Vec<u8> {
        (0..N as u32).map(|i| ((i.wrapping_mul(2654435761) ^ seed.wrapping_mul(40503)) >> 7) as u8).collect()
    }

    fn original(len: usize) -> Prints {
        (0..len).map(|k| (k as f64, frame(k as u32))).collect()
    }

    /// 8 frames a second from `from` for `secs`, each the nearest keyframe's picture.
    fn cut(from: usize, secs: usize) -> Prints {
        (0..secs * 8).map(|k| (k as f64 / 8.0, frame((from + k / 8) as u32))).collect()
    }

    #[test]
    fn a_cut_matches_its_original() {
        assert!(cut_from(&original(180), &cut(126, 20)));
    }

    #[test]
    fn a_cut_from_another_clip_doesnt() {
        let other: Prints = (0..160).map(|k| (k as f64 / 8.0, frame(1000 + k))).collect();
        assert!(!cut_from(&original(180), &other));
    }

    #[test]
    fn flat_frames_dont_count() {
        // Both all black (a loading screen): says nothing.
        let black = |n: usize, step: f64| (0..n).map(|k| (k as f64 * step, vec![0u8; N])).collect::<Prints>();
        assert!(!cut_from(&black(180, 1.0), &black(160, 0.125)));
    }

    #[test]
    fn one_matching_frame_isnt_enough() {
        // Only its first two seconds are from the original.
        let mut c = cut(50, 2);
        c.extend((16..160).map(|k| (k as f64 / 8.0, frame(5000 + k))));
        assert!(!cut_from(&original(180), &c));
    }
}

#[cfg(test)]
mod real {
    /// Real clips: HINDSIGHT_TRIM_CANDIDATES = a JSON file of candidates. Prints the matches.
    #[test]
    #[ignore]
    fn match_real_clips() {
        let Ok(f) = std::env::var("HINDSIGHT_TRIM_CANDIDATES") else { return };
        crate::ffmpeg::init(None);
        let list: Vec<super::Candidate> = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let cache = std::env::temp_dir().join("trim-matches-test.json");
        let _ = std::fs::remove_file(&cache);
        let t = std::time::Instant::now();
        let out = super::check(&cache, &list, &mut |d, n| eprintln!("{d}/{n} {:?}", t.elapsed()), &mut |o, e| eprintln!("MATCH {o} -> {e}"), &|| false);
        eprintln!("{} of {} matched in {:?}", out.len(), list.len(), t.elapsed());
    }
}
