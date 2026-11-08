//! Clip library: listing files, probing, thumbnails, and per-file tools
//! (remux, trim, export, compress). Originals are never modified.

use crate::audio_edit::AudioEdit;
use crate::ffmpeg;
use serde::Serialize;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const VIDEO_EXT: &[&str] = &["mkv", "mp4", "mov", "webm", "m4v", "avi", "ts", "flv"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipFile {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub modified: i64,
    pub external: bool,
    /// "local" (this PC), "nas" (only on the NAS) or "linked" (another recorder's folder)
    pub location: &'static str,
    /// A local clip that's also safely on the NAS
    pub on_nas: bool,
    /// Where it came from, for a badge: "" (Hindsight), "NAS", "Medal", "OBS", "NVIDIA" or a folder name
    pub source: String,
}

/// Another recorder's folder shown in the library in place.
pub struct LinkedRoot {
    pub path: PathBuf,
    pub label: String,
}

#[derive(Serialize, serde::Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub duration: f64,
    pub codec: String,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub audio_tracks: u32,
    pub container: String,
}

/// Medal can save each clip's sound as extra files next to it:
/// "MedalTVValorant20250914010640_Game.mp4" and "…_Mic.mp4" beside
/// "MedalTVValorant20250914010640.mp4". They're sound only, part of that clip,
/// not clips of their own.
pub(crate) fn is_sidecar(p: &Path) -> bool {
    let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return false };
    let Some(main) = ["_Game", "_Mic", "_Desktop", "_Voice"].iter().find_map(|s| stem.strip_suffix(s)) else { return false };
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    p.with_file_name(format!("{main}.{ext}")).is_file()
}

pub(crate) fn is_video(p: &Path) -> bool {
    // ".name.part.mkv": an edit still being written (see `part_of`), not a clip yet.
    if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
        return false;
    }
    p.extension()
        .map(|e| VIDEO_EXT.contains(&e.to_string_lossy().to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Where an edit is written until it's finished: a hidden ".name.part.ext" next
/// to its final name, so the library never shows a half-written file (it used
/// to appear as a 0:00 clip while saving).
fn part_of(out: &Path) -> PathBuf {
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = out.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    out.with_file_name(format!(".{stem}.part.{ext}"))
}

/// Move a finished edit from its hidden name to its real one.
fn publish(part: &Path, out: &Path) -> Result<String, String> {
    if fs::metadata(part).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = fs::remove_file(part);
        return Err("The edit came out empty, so nothing was saved.".into());
    }
    fs::rename(part, out).map_err(|e| {
        let _ = fs::remove_file(part);
        format!("Couldn't save the edit: {e}")
    })?;
    Ok(out.to_string_lossy().into_owned())
}

fn clip_file(p: &Path, folder: String, external: bool) -> Option<ClipFile> {
    let meta = fs::metadata(p).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(ClipFile {
        path: p.to_string_lossy().into_owned(),
        name: p.file_stem()?.to_string_lossy().into_owned(),
        folder,
        size: meta.len(),
        modified,
        external,
        location: "local",
        on_nas: false,
        source: String::new(),
    })
}

/// Folders inside another recorder's folder that hold no clips.
fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || name.to_lowercase().contains("thumbnail") || name.eq_ignore_ascii_case("cache")
}

/// Videos up to three folders deep (Medal: Clips/<Game>/; NVIDIA: <Game>/; OBS: flat).
/// The group is the folder a file sits in, or the recorder's name for files at the top.
pub(crate) fn scan_linked(root: &LinkedRoot, avoid: &[PathBuf], out: &mut Vec<ClipFile>) {
    fn walk(dir: &Path, depth: u32, root: &LinkedRoot, avoid: &[PathBuf], out: &mut Vec<ClipFile>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if depth < 3 && !skip_dir(&name) && !avoid.iter().any(|a| same_path(a, &p)) {
                    walk(&p, depth + 1, root, avoid, out);
                }
            } else if is_video(&p) && !is_sidecar(&p) {
                let group = if depth == 0 { root.label.clone() } else { dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() };
                if let Some(mut c) = clip_file(&p, group, true) {
                    c.location = "linked";
                    c.source = root.label.clone();
                    out.push(c);
                }
            }
        }
    }
    walk(&root.path, 0, root, avoid, out);
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}

/// The whole library: this PC's clips, clips only on the NAS (e.g. from other
/// PCs), other recorders' folders, and files opened from elsewhere.
pub fn scan_all(clips_dir: &Path, external: &[String], nas: &[crate::nas::NasFile], linked: &[LinkedRoot], nas_dir: &str) -> Vec<ClipFile> {
    let mut out = scan(clips_dir, external);
    let key = |s: &str| s.replace('\\', "/").to_lowercase();
    let local_rels: std::collections::HashMap<String, usize> = out
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.external)
        .filter_map(|(i, c)| Path::new(&c.path).strip_prefix(clips_dir).ok().map(|r| (key(&r.to_string_lossy()), i)))
        .collect();
    for f in nas {
        match local_rels.get(&key(&f.rel)) {
            Some(&i) => out[i].on_nas = true,
            None => {
                if let Some(mut c) = clip_file(&f.path, f.folder.clone(), false) {
                    c.location = "nas";
                    c.source = "NAS".into();
                    out.push(c);
                }
            }
        }
    }
    let mut avoid = vec![clips_dir.to_path_buf()];
    if !nas_dir.is_empty() {
        avoid.push(PathBuf::from(nas_dir));
    }
    avoid.extend(linked.iter().map(|l| l.path.clone()));
    let mut seen: std::collections::HashSet<String> = out.iter().map(|c| c.path.to_lowercase()).collect();
    for root in linked {
        let mut found = Vec::new();
        let others: Vec<PathBuf> = avoid.iter().filter(|a| !same_path(a, &root.path)).cloned().collect();
        scan_linked(root, &others, &mut found);
        out.extend(found.into_iter().filter(|c| seen.insert(c.path.to_lowercase())));
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Clips folder (and one level of game subfolders) plus files opened from elsewhere.
pub fn scan(clips_dir: &Path, external: &[String]) -> Vec<ClipFile> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(clips_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let folder = e.file_name().to_string_lossy().into_owned();
                if folder.starts_with('.') {
                    continue; // in-progress recordings and other hidden folders
                }
                if let Ok(inner) = fs::read_dir(&p) {
                    for f in inner.flatten() {
                        let fp = f.path();
                        if fp.is_file() && is_video(&fp) && !is_sidecar(&fp) {
                            out.extend(clip_file(&fp, folder.clone(), false));
                        }
                    }
                }
            } else if is_video(&p) && !is_sidecar(&p) {
                out.extend(clip_file(&p, "Other".into(), false));
            }
        }
    }
    for ext in external {
        let p = Path::new(ext);
        if p.starts_with(clips_dir) {
            continue;
        }
        out.extend(clip_file(p, "Imported".into(), true));
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

fn parse_rate(r: &str) -> f64 {
    match r.split_once('/') {
        Some((n, d)) => {
            let (n, d) = (n.parse::<f64>().unwrap_or(0.0), d.parse::<f64>().unwrap_or(0.0));
            if d > 0.0 { n / d } else { 0.0 }
        }
        None => r.parse().unwrap_or(0.0),
    }
}

pub fn probe(path: &str) -> Result<Probe, String> {
    let mut args = ffmpeg::args(&[
        "-v",
        "error",
        "-show_entries",
        "format=duration,format_name:stream=codec_type,codec_name,width,height,avg_frame_rate",
        "-of",
        "json",
    ]);
    args.push(path.into());
    let json: Value = serde_json::from_str(&ffmpeg::run_limited("ffprobe", &args, Some(std::time::Duration::from_secs(45)))?).map_err(|e| e.to_string())?;
    let mut p = Probe {
        duration: json["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0),
        container: json["format"]["format_name"].as_str().unwrap_or("").to_string(),
        ..Default::default()
    };
    for s in json["streams"].as_array().into_iter().flatten() {
        match s["codec_type"].as_str() {
            Some("video") if p.codec.is_empty() => {
                p.codec = s["codec_name"].as_str().unwrap_or("").to_string();
                p.width = s["width"].as_i64().unwrap_or(0);
                p.height = s["height"].as_i64().unwrap_or(0);
                p.fps = parse_rate(s["avg_frame_rate"].as_str().unwrap_or("0"));
            }
            Some("audio") => p.audio_tracks += 1,
            _ => {}
        }
    }
    Ok(p)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTrack {
    /// Audio track number in the file (1 = second track; 0 plays in the video itself)
    pub index: u32,
    pub title: String,
    pub path: String,
}

/// The video player only plays a file's first audio track. Every other track
/// (e.g. the mic) is pulled out into its own small audio file so Studio can
/// play it alongside, in sync. Copied without re-encoding when possible.
pub fn preview_tracks(path: &str, cache: &Path) -> Result<Vec<PreviewTrack>, String> {
    let mut args = ffmpeg::args(&["-v", "error", "-select_streams", "a", "-show_entries", "stream=codec_name:stream_tags=title", "-of", "json"]);
    args.push(path.into());
    let json: Value = serde_json::from_str(&ffmpeg::run("ffprobe", &args)?).map_err(|e| e.to_string())?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    let key = format!("{:016x}", h.finish());
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for (i, s) in streams.iter().enumerate().skip(1) {
        let title = s["tags"]["title"].as_str().map(str::to_string).unwrap_or_else(|| {
            if i == 1 { "Microphone".into() } else { format!("Track {}", i + 1) }
        });
        let file = cache.join(format!("{key}_a{i}.m4a"));
        if !file.exists() {
            let codec = s["codec_name"].as_str().unwrap_or("");
            let tmp = cache.join(format!("{key}_a{i}.part.m4a"));
            let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-i"]);
            a.push(path.into());
            a.extend(ffmpeg::args(&["-map", &format!("0:a:{i}"), "-vn"]));
            if codec == "aac" {
                a.extend(ffmpeg::args(&["-c:a", "copy"]));
            } else {
                a.extend(ffmpeg::args(&["-c:a", "aac", "-b:a", "160k"]));
            }
            a.extend(ffmpeg::args(&["-movflags", "+faststart"]));
            a.push(tmp.clone().into());
            ffmpeg::run("ffmpeg", &a)?;
            fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
        }
        out.push(PreviewTrack { index: i as u32, title, path: file.to_string_lossy().into_owned() });
    }
    Ok(out)
}

/// The first audio track's title (what plays inside the video element).
pub fn first_track_title(path: &str) -> String {
    let mut args = ffmpeg::args(&["-v", "error", "-select_streams", "a:0", "-show_entries", "stream_tags=title", "-of", "json"]);
    args.push(path.into());
    ffmpeg::run("ffprobe", &args)
        .ok()
        .and_then(|j| serde_json::from_str::<Value>(&j).ok())
        .and_then(|v| v["streams"][0]["tags"]["title"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Game".into())
}

fn thumbnail_path(path: &str, cache: &Path) -> Result<PathBuf, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    Ok(thumbnail_key(path, &meta, cache))
}

fn thumbnail_key(path: &str, meta: &fs::Metadata, cache: &Path) -> PathBuf {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    cache.join(format!("{:016x}.jpg", h.finish()))
}

/// The NAS folder and this PC's clips folder, so a clip on the NAS can find
/// what was already worked out for it while it was on this PC.
static TWIN: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

pub fn set_twin(nas_dir: &str, clips_dir: &str) {
    *TWIN.lock().unwrap() = (!nas_dir.is_empty()).then(|| (format!("{}\\", nas_dir.trim_end_matches(['\\', '/']).to_lowercase()), clips_dir.trim_end_matches(['\\', '/']).to_string()));
}

/// For a clip on the NAS: where its copy on this PC is, or was before it was
/// freed up (same game folder and name). Its size and date are the same there,
/// so its thumbnail and details can be reused instead of read over the network.
pub fn local_twin(path: &str) -> Option<String> {
    let t = TWIN.lock().unwrap();
    let (nas, local) = t.as_ref()?;
    let p = path.replace('/', "\\");
    let rest = if p.to_lowercase().starts_with(nas.as_str()) { p.get(nas.len()..)? } else { return None };
    Some(format!("{local}\\{rest}"))
}

/// A thumbnail made for this clip's twin on this PC, if there is one.
fn twin_thumbnail(path: &str, cache: &Path) -> Option<PathBuf> {
    let twin = local_twin(path)?;
    let meta = fs::metadata(path).ok()?;
    let old = thumbnail_key(&twin, &meta, cache);
    old.exists().then_some(old)
}

/// Is this clip's thumbnail already made?
pub fn thumbnail_exists(path: &str, cache: &Path) -> bool {
    thumbnail_path(path, cache).is_ok_and(|p| p.exists()) || twin_thumbnail(path, cache).is_some()
}

pub fn thumbnail(path: &str, cache: &Path) -> Result<String, String> {
    let out = thumbnail_path(path, cache)?;
    if out.exists() {
        return Ok(out.to_string_lossy().into_owned());
    }
    if let Some(old) = twin_thumbnail(path, cache) {
        return Ok(old.to_string_lossy().into_owned());
    }
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    for seek in ["1", "0"] {
        let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-ss", seek, "-i"]);
        args.push(path.into());
        args.extend(ffmpeg::args(&["-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "4"]));
        args.push(out.clone().into());
        // A stuck ffmpeg (a drive that stopped answering) mustn't hold the thumbnail queue.
        if ffmpeg::run_limited("ffmpeg", &args, Some(std::time::Duration::from_secs(20))).is_ok() && out.exists() {
            return Ok(out.to_string_lossy().into_owned());
        }
    }
    Err("Couldn't make a thumbnail".into())
}

/// "<dir>/<stem><suffix>.<ext>", adding (2), (3)… if it already exists.
fn unique(src: &Path, suffix: &str, ext: &str) -> PathBuf {
    let dir = src.parent().unwrap_or(Path::new("."));
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut p = dir.join(format!("{stem}{suffix}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}{suffix} ({n}).{ext}"));
        n += 1;
    }
    p
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn nvenc_for(codec: &str) -> &'static str {
    match codec {
        "av1" => "av1_nvenc",
        "hevc" => "hevc_nvenc",
        _ => "h264_nvenc",
    }
}

/// Run ffmpeg into `out` (the last argument): written under a hidden name
/// first, and only given its real name once it's finished.
fn finish(mut args: Vec<OsString>, out: PathBuf) -> Result<String, String> {
    let part = part_of(&out);
    if args.last().map(|a| Path::new(a) == out).unwrap_or(false) {
        args.pop();
        args.push(part.clone().into());
    } else {
        return run_into(args, out);
    }
    match ffmpeg::run("ffmpeg", &args) {
        Ok(_) => publish(&part, &out),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

fn run_into(args: Vec<OsString>, out: PathBuf) -> Result<String, String> {
    match ffmpeg::run("ffmpeg", &args) {
        Ok(_) => Ok(out.to_string_lossy().into_owned()),
        Err(e) => {
            let _ = fs::remove_file(&out);
            Err(e)
        }
    }
}

/// The whole clip with only its audio changed: the video is copied as it is
/// and only the audio is redone, so it takes seconds instead of re-encoding
/// the whole video (which took about a minute for a 3-minute clip).
fn audio_only(path: &str, out: &Path, audio: &AudioEdit) -> Result<String, String> {
    let n = probe(path).map(|p| p.audio_tracks).unwrap_or(0) as usize;
    if n == 0 {
        return Err("This clip has no audio to change.".into());
    }
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-i"]);
    args.push(path.into());
    let graph: Vec<String> = (0..n).map(|i| format!("{}anull[a{i}]", audio.chain(i, 0.0))).collect();
    args.extend(ffmpeg::args(&["-filter_complex", &graph.join(";"), "-map", "0:v:0"]));
    for i in 0..n {
        args.extend(ffmpeg::args(&["-map", &format!("[a{i}]"), &format!("-map_metadata:s:a:{i}"), &format!("0:s:a:{i}")]));
    }
    args.extend(ffmpeg::args(&["-c:v", "copy", "-c:a", "aac", "-b:a", "192k"]));
    if ext_of(out) == "mp4" {
        args.extend(ffmpeg::args(&["-movflags", "+faststart"]));
    }
    args.push(out.to_path_buf().into());
    finish(args, out.to_path_buf())
}

/// Same video/audio, new MP4 container. No quality loss, takes a second.
pub fn remux_mp4(path: &str, keep_original: bool) -> Result<String, String> {
    let src = Path::new(path);
    let out = unique(src, if ext_of(src) == "mp4" { " (remux)" } else { "" }, "mp4");
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-i"]);
    args.push(path.into());
    args.extend(ffmpeg::args(&["-map", "0", "-c", "copy", "-movflags", "+faststart"]));
    args.push(out.clone().into());
    let result = finish(args, out)?;
    if !keep_original {
        let original_secs = probe(path).map(|p| p.duration).ok();
        verify_output(Path::new(&result), original_secs)?;
        discard(src).map_err(|e| format!("Remuxed, but couldn't recycle the original: {e}"))?;
    }
    Ok(result)
}

/// Fast = cut on keyframes, no re-encode. Exact = frame-accurate, re-encoded on NVENC.
/// Trim to [start, end] seconds. `exact` is frame-accurate: a "smart cut"
/// that re-encodes only the frames before the first keyframe in the range
/// (under a second for Hindsight's clips) and copies everything after it
/// untouched, so it's nearly as fast as a plain copy and keeps full quality.
/// If the file isn't suited to that, or any check fails, the whole range is
/// re-encoded instead. Without `exact`, the cut snaps to a keyframe.
pub fn trim(path: &str, start: f64, end: f64, exact: bool, audio: &AudioEdit) -> Result<String, String> {
    if end <= start {
        return Err("The end point has to be after the start point.".into());
    }
    let src = Path::new(path);
    let ext = match ext_of(src).as_str() {
        "mkv" | "mp4" | "webm" | "mov" => ext_of(src),
        _ => "mp4".into(),
    };
    let out = unique(src, " (trim)", &ext);
    // Nothing cut, only the sound changed: copy the video, redo the audio.
    let whole = start <= 0.01 && probe(path).map(|p| p.duration > 0.0 && end >= p.duration - 0.05).unwrap_or(false);
    if whole && !audio.is_empty() {
        return audio_only(path, &out, audio);
    }
    if exact {
        let part = part_of(&out);
        match smart_trim(path, start, end, &part, audio) {
            Ok(()) => return publish(&part, &out),
            Err(e) => {
                crate::log!("[rewind] smart trim not used ({e}); re-encoding the whole range");
                let _ = fs::remove_file(&part);
            }
        }
    }
    let (ss, to) = (format!("{start:.3}"), format!("{end:.3}"));
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-ss", &ss, "-to", &to, "-i"]);
    args.push(path.into());
    // Changed audio has to be re-encoded, so a keyframe cut becomes an exact one.
    let exact = exact || !audio.is_empty();
    args.extend(ffmpeg::args(&["-map", "0:v:0"]));
    if audio.is_empty() {
        args.extend(ffmpeg::args(&["-map", "0:a?"]));
    } else {
        // The input starts at the cut (-ss before -i), so mute times shift by `start`.
        let n = probe(path).map(|p| p.audio_tracks).unwrap_or(0) as usize;
        let graph: Vec<String> = (0..n).map(|i| format!("{}anull[a{i}]", audio.chain(i, start))).collect();
        if n > 0 {
            args.extend(ffmpeg::args(&["-filter_complex", &graph.join(";")]));
        }
        for i in 0..n {
            args.extend(ffmpeg::args(&["-map", &format!("[a{i}]"), &format!("-map_metadata:s:a:{i}"), &format!("0:s:a:{i}")]));
        }
    }
    if exact {
        let p = probe(path).unwrap_or_default();
        let gop = (p.fps.round() as i64).max(1).to_string();
        // Audio is re-encoded too: copied audio can't be cut mid-packet and drifts out of sync.
        // A keyframe every second keeps later trims of this file quick.
        args.extend(ffmpeg::args(&[
            "-c:v", nvenc_for(&p.codec), "-preset", "p5", "-cq", "20", "-b:v", "0", "-g", &gop, "-c:a", "aac", "-b:a", "192k",
        ]));
    } else {
        args.extend(ffmpeg::args(&["-c", "copy", "-avoid_negative_ts", "make_zero"]));
    }
    if ext == "mp4" {
        args.extend(ffmpeg::args(&["-movflags", "+faststart"]));
    }
    args.push(out.clone().into());
    finish(args, out)
}

/// What a smart cut needs to know about the video stream.
struct CutInfo {
    codec: String,
    /// Raw bitstream format for this codec ("obu", "hevc", "h264").
    raw: &'static str,
    fps: f64,
    /// Time of the first video frame (Hindsight's clips start at one frame in).
    first: f64,
    audio: usize,
    /// Codec level as ffprobe reports it (AV1: seq_level_idx, 31 = none; H.264: 41 = 4.1; HEVC: 123 = 4.1)
    level: i64,
    profile: String,
    /// What has to match between the re-encoded start and the copied rest
    /// (otherwise strict players, like Windows' hardware AV1 decoder, refuse the file).
    sig: String,
}

/// The stream properties a decoder sets itself up from.
fn stream_sig(v: &Value) -> String {
    let s = |k: &str| v[k].as_str().map(str::to_string).or_else(|| v[k].as_i64().map(|n| n.to_string())).unwrap_or_default();
    ["profile", "level", "pix_fmt", "color_range", "color_space", "color_primaries", "color_transfer", "width", "height"]
        .iter()
        .map(|k| {
            let mut val = s(k);
            // Unmarked colour info means the default: limited ("tv") range, unspecified primaries.
            if val == "unknown" || val == "unspecified" || val == "reserved" {
                val.clear();
            }
            if *k == "color_range" && val.is_empty() {
                val = "tv".into();
            }
            format!("{k}={val}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// NVENC options that make a re-encode match the source's profile and level.
fn match_level_args(ci: &CutInfo) -> Vec<String> {
    let mut a = Vec::new();
    match ci.codec.as_str() {
        // av1_nvenc's level numbers are seq_level_idx; 31 means "no level" = NVENC's auto.
        "av1" if (0..24).contains(&ci.level) => a.extend(["-level".to_string(), ci.level.to_string()]),
        "hevc" | "h264" if ci.level > 0 => a.extend(["-level".to_string(), ci.level.to_string()]),
        _ => {}
    }
    if ci.codec == "h264" {
        let p = match ci.profile.to_lowercase().as_str() {
            "main" => "main",
            p if p.contains("baseline") => "baseline",
            _ => "high",
        };
        a.extend(["-profile:v".to_string(), p.to_string()]);
    }
    a
}

fn cut_info(path: &str) -> Result<CutInfo, String> {
    let mut a = ffmpeg::args(&["-v", "error", "-print_format", "json", "-show_streams", "-i"]);
    a.push(path.into());
    let json: Value = serde_json::from_str(&ffmpeg::run("ffprobe", &a)?).map_err(|e| e.to_string())?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let v = streams.iter().find(|s| s["codec_type"] == "video").ok_or("no video")?;
    let codec = v["codec_name"].as_str().unwrap_or("").to_string();
    let raw = match codec.as_str() {
        "av1" => "obu",
        "hevc" => "hevc",
        "h264" => "h264",
        _ => return Err(format!("codec {codec}")),
    };
    let pix = v["pix_fmt"].as_str().unwrap_or("");
    if pix != "yuv420p" && pix != "nv12" {
        return Err(format!("pixel format {pix}"));
    }
    let rate = |k: &str| -> f64 {
        let r = v[k].as_str().unwrap_or("0/1");
        let (n, d) = r.split_once('/').unwrap_or((r, "1"));
        n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(1.0).max(1e-9)
    };
    let (fps, avg) = (rate("r_frame_rate"), rate("avg_frame_rate"));
    // Raw streams get their timing back from a fixed frame rate, so only
    // constant-frame-rate files qualify (all of Hindsight's own clips are).
    if fps <= 0.0 || (avg - fps).abs() > fps * 0.002 {
        return Err(format!("variable frame rate ({fps:.3} vs {avg:.3})"));
    }
    let first = v["start_time"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let audio = streams.iter().filter(|s| s["codec_type"] == "audio").count();
    let level = v["level"].as_i64().unwrap_or(-99);
    let profile = v["profile"].as_str().unwrap_or("").to_string();
    Ok(CutInfo { codec, raw, fps, first, audio, level, profile, sig: stream_sig(v) })
}

/// Frame numbers (from the first frame) of the keyframes between `from` and `to` seconds.
fn keyframes(path: &str, ci: &CutInfo, from: f64, to: f64) -> Result<Vec<i64>, String> {
    // ffprobe starts reading at the keyframe before `from`; read on past `to`
    // so even widely spaced keyframes are found.
    let interval = format!("{:.3}%{:.3}", (from - 1.0).max(0.0), to + 1.0);
    let mut a = ffmpeg::args(&["-v", "error", "-select_streams", "v:0", "-read_intervals", &interval]);
    a.extend(ffmpeg::args(&["-show_entries", "packet=pts_time,flags", "-of", "csv=p=0", "-i"]));
    a.push(path.into());
    Ok(ffmpeg::run("ffprobe", &a)?
        .lines()
        .filter(|l| l.contains(",K"))
        .filter_map(|l| l.split(',').next()?.parse::<f64>().ok())
        .map(|t| ((t - ci.first) * ci.fps).round() as i64)
        .collect())
}

fn smart_trim(path: &str, start: f64, end: f64, out: &Path, audio: &AudioEdit) -> Result<(), String> {
    let ci = cut_info(path)?;
    let fps = ci.fps;
    // Work in frame numbers so rounding can't add or drop a frame.
    // The frame on screen at `start` is the first kept; frames before `end` are kept.
    let i_start = (((start - ci.first) * fps) + 1e-3).floor().max(0.0) as i64;
    let i_end = (((end - ci.first) * fps) - 1e-3).ceil() as i64;
    if i_end <= i_start {
        return Err("range shorter than a frame".into());
    }
    let key = keyframes(path, &ci, start, end)?.into_iter().filter(|k| *k >= i_start && *k < i_end).min();
    let i_key = key.unwrap_or(i_end); // no keyframe inside: re-encode it all
    let at = |i: i64| format!("{:.6}", ci.first + i as f64 / fps);
    // Half a frame early, so a cut lands cleanly between two frames.
    let before = |i: i64| format!("{:.6}", ci.first + (i as f64 - 0.5) / fps);
    let pre = |i: i64| format!("{:.3}", (ci.first + i as f64 / fps - 2.0).max(0.0));

    let dir = std::env::temp_dir().join(format!("rewind-trim-{}-{}", std::process::id(), chrono::Local::now().format("%H%M%S%f")));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        // Audio (all tracks, sample-exact) runs alongside the video steps.
        let audio_path = dir.join("audio.mka");
        let audio_job = (ci.audio > 0).then(|| {
            let mut graph = Vec::new();
            for i in 0..ci.audio {
                // Volume and mutes first, on the original's timeline.
                graph.push(format!("{}atrim=start={}:end={},asetpts=PTS-STARTPTS[a{i}]", audio.chain(i as usize, 0.0), at(i_start), at(i_end)));
            }
            let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-i"]);
            a.push(path.into());
            a.extend(ffmpeg::args(&["-filter_complex", &graph.join(";")]));
            for i in 0..ci.audio {
                a.extend(ffmpeg::args(&["-map", &format!("[a{i}]"), &format!("-map_metadata:s:a:{i}"), &format!("0:s:a:{i}")]));
            }
            a.extend(ffmpeg::args(&["-c:a", "aac", "-b:a", "192k", "-f", "matroska"]));
            a.push(audio_path.clone().into());
            std::thread::spawn(move || ffmpeg::run("ffmpeg", &a))
        });

        // 1. Frames before the first keyframe: re-encoded, near-lossless.
        let head = dir.join("head.raw");
        if i_key > i_start {
            let gop = (fps.round() as i64).max(1).to_string();
            let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-copyts", "-ss", &pre(i_start), "-i"]);
            a.push(path.into());
            a.extend(ffmpeg::args(&["-map", "0:v:0", "-an", "-vf", &format!("trim=start={}:end={}", before(i_start), before(i_key))]));
            a.extend(ffmpeg::args(&[
                "-c:v", nvenc_for(&ci.codec), "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "16", "-b:v", "0", "-bf", "0", "-g", &gop,
                "-pix_fmt", "yuv420p",
            ]));
            // Same level and profile as the original, so the stream doesn't change halfway.
            a.extend(match_level_args(&ci).into_iter().map(OsString::from));
            a.extend(ffmpeg::args(&["-f", ci.raw]));
            a.push(head.clone().into());
            ffmpeg::run("ffmpeg", &a)?;
            // Joining two different-looking streams makes a file some players refuse
            // (e.g. an OBS clip at AV1 level 4.1 with a start at "no level"):
            // if anything still differs, re-encode the whole range instead.
            if i_key < i_end {
                let mut p = ffmpeg::args(&["-v", "error", "-f", ci.raw, "-print_format", "json", "-show_streams", "-i"]);
                p.push(head.clone().into());
                let j: Value = serde_json::from_str(&ffmpeg::run("ffprobe", &p)?).map_err(|e| e.to_string())?;
                let got = j["streams"].get(0).map(stream_sig).unwrap_or_default();
                if got != ci.sig {
                    return Err(format!("the re-encoded start wouldn't match the rest ({got} vs {})", ci.sig));
                }
            }
        }
        // 2. From the keyframe on: copied untouched.
        let tail = dir.join("tail.raw");
        if i_key < i_end {
            let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-copyts", "-ss", &pre(i_key), "-i"]);
            a.push(path.into());
            a.extend(ffmpeg::args(&["-ss", &before(i_key), "-to", &before(i_end), "-map", "0:v:0", "-an", "-c", "copy", "-f", ci.raw]));
            a.push(tail.clone().into());
            ffmpeg::run("ffmpeg", &a)?;
        }
        // 3. One video stream (raw bitstreams join end to end), then mux with the audio.
        let video = dir.join("video.raw");
        {
            let mut v = fs::read(&head).unwrap_or_default();
            v.extend(fs::read(&tail).unwrap_or_default());
            fs::write(&video, v).map_err(|e| e.to_string())?;
        }
        if let Some(job) = audio_job {
            job.join().map_err(|_| "audio step crashed".to_string())??;
        }
        let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-r", &format!("{fps}"), "-f", ci.raw, "-i"]);
        a.push(video.clone().into());
        if ci.audio > 0 {
            a.push("-i".into());
            a.push(audio_path.clone().into());
        }
        a.extend(ffmpeg::args(&["-map", "0:v"]));
        if ci.audio > 0 {
            a.extend(ffmpeg::args(&["-map", "1:a"]));
        }
        a.extend(ffmpeg::args(&["-c", "copy"]));
        if ext_of(out) == "mp4" {
            a.extend(ffmpeg::args(&["-movflags", "+faststart"]));
        }
        a.push(out.into());
        ffmpeg::run("ffmpeg", &a)?;

        // 4. Checks: the right number of frames, and clean decoding across the join.
        let mut a = ffmpeg::args(&["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", "-i"]);
        a.push(out.into());
        let frames: i64 = ffmpeg::run("ffprobe", &a)?.trim().parse().map_err(|_| "couldn't count frames".to_string())?;
        if frames != i_end - i_start {
            return Err(format!("{frames} frames instead of {}", i_end - i_start));
        }
        let join = (i_key - i_start) as f64 / fps;
        let mut a = ffmpeg::args(&["-hide_banner", "-v", "error", "-xerror", "-ss", &format!("{:.3}", (join - 1.0).max(0.0)), "-t", "2.5", "-i"]);
        a.push(out.into());
        a.extend(ffmpeg::args(&["-f", "null", "-"]));
        ffmpeg::run("ffmpeg", &a).map_err(|e| format!("decode check failed: {e}"))?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}

/// Map video plus one audio track: desktop + mic mixed when there are several,
/// because most players and Discord only play the first track.
fn map_mixed_audio(args: &mut Vec<OsString>, audio_tracks: u32, audio: &AudioEdit) {
    args.extend(ffmpeg::args(&["-map", "0:v:0"]));
    match audio_tracks {
        0 => {}
        1 if audio.is_empty() => args.extend(ffmpeg::args(&["-map", "0:a:0"])),
        1 => {
            let graph = format!("{}anull[mix]", audio.chain(0, 0.0));
            args.extend(ffmpeg::args(&["-filter_complex", &graph, "-map", "[mix]"]));
        }
        n => {
            let mut parts: Vec<String> = (0..n as usize).map(|i| format!("{}anull[t{i}]", audio.chain(i, 0.0))).collect();
            let inputs: String = (0..n).map(|i| format!("[t{i}]")).collect();
            parts.push(format!("{inputs}amix=inputs={n}:duration=longest:normalize=0[mix]"));
            args.extend(ffmpeg::args(&["-filter_complex", &parts.join(";"), "-map", "[mix]"]));
        }
    }
}

/// Can this file really go to the Recycle Bin? Windows quietly deletes for good
/// when the drive's bin is set to "Don't move files to the Recycle Bin", or the
/// file is bigger than the bin's maximum size. Either way it would be gone, so
/// Hindsight refuses instead. Unknown settings (no entry) count as fine.
pub fn recycle_check(path: &Path) -> Result<(), String> {
    use windows::Win32::Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    unsafe {
        let mut root = [0u16; 260];
        if GetVolumePathNameW(&HSTRING::from(path.as_os_str()), &mut root).is_err() {
            return Ok(());
        }
        let mut vol = [0u16; 64];
        if GetVolumeNameForVolumeMountPointW(PCWSTR(root.as_ptr()), &mut vol).is_err() {
            return Ok(());
        }
        let vol = String::from_utf16_lossy(&vol);
        let Some(guid) = vol.find('{').and_then(|a| vol[a..].find('}').map(|b| vol[a..a + b + 1].to_string())) else { return Ok(()) };
        let key = HSTRING::from(format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}"));
        let dword = |name: &str| {
            let (mut v, mut len) = (0u32, 4u32);
            RegGetValueW(HKEY_CURRENT_USER, &key, &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut len))
                .ok()
                .ok()
                .map(|_| v)
        };
        if dword("NukeOnDelete") == Some(1) {
            return Err("This drive's Recycle Bin is set to delete files straight away, so it would be gone for good. Nothing was deleted.".into());
        }
        if let Some(mb) = dword("MaxCapacity")
            && mb > 0
            && size > mb as u64 * 1_048_576
        {
            return Err(format!(
                "It's bigger than this drive's Recycle Bin ({:.1} GB max), so Windows would delete it for good. Nothing was deleted.",
                mb as f64 / 1024.0
            ));
        }
    }
    Ok(())
}

/// Send a file to the Recycle Bin (tests delete outright), but only if it can
/// really land there.
pub(crate) fn discard(p: &Path) -> Result<(), String> {
    #[cfg(test)]
    return fs::remove_file(p).map_err(|e| e.to_string());
    #[cfg(not(test))]
    {
        recycle_check(p)?;
        trash::delete(p).map_err(|e| e.to_string())
    }
}

/// Safety check before touching an original: the new file must be a real,
/// playable video of a sensible length.
fn verify_output(edited: &Path, expected_secs: Option<f64>) -> Result<(), String> {
    let size = fs::metadata(edited).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err("The edit came out empty, so the original was kept.".into());
    }
    let p = probe(&edited.to_string_lossy()).map_err(|_| "The edit couldn't be read back, so the original was kept.".to_string())?;
    if p.codec.is_empty() || p.duration <= 0.05 {
        return Err("The edit has no video, so the original was kept.".into());
    }
    if let Some(want) = expected_secs
        && (p.duration - want).abs() > want.max(1.0) * 0.25 + 1.5
    {
        return Err(format!(
            "The edit is {:.1}s but should be about {:.1}s, so the original was kept.",
            p.duration, want
        ));
    }
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub deleted: Vec<String>,
    /// (path, reason)
    pub failed: Vec<(String, String)>,
}

/// Only videos that are actually in the library can be deleted: inside the
/// clips folder, or a file the user opened from elsewhere.
fn deletable(path: &Path, clips_dir: &Path, external: &[String], linked: &[String], nas_dir: &str) -> Result<(), String> {
    if !is_video(path) {
        return Err("Not a video file".into());
    }
    let within = |root: &Path| match (path.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(c)) => p.starts_with(c),
        _ => false,
    };
    // Network folders have no Recycle Bin, so a delete there couldn't be undone.
    let on_network = path.to_string_lossy().starts_with(r"\\") || (!nas_dir.is_empty() && within(Path::new(nas_dir)));
    if on_network {
        return Err("It's on the NAS. Network folders have no Recycle Bin, so delete it on the NAS itself.".into());
    }
    let inside = within(clips_dir) || linked.iter().any(|l| within(Path::new(l)));
    let listed = external.iter().any(|e| Path::new(e) == path);
    if inside || listed { Ok(()) } else { Err("Not part of your Hindsight library".into()) }
}

/// Move clips to the Recycle Bin (one batch, so Windows can restore them together).
pub fn delete_clips(paths: &[String], clips_dir: &Path, external: &[String], linked: &[String], nas_dir: &str) -> DeleteResult {
    let mut res = DeleteResult::default();
    let mut ok: Vec<PathBuf> = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if !path.exists() {
            res.deleted.push(p.clone()); // already gone: treat as done
            continue;
        }
        match deletable(path, clips_dir, external, linked, nas_dir).and_then(|_| if cfg!(test) { Ok(()) } else { recycle_check(path) }) {
            Ok(()) => ok.push(path.to_path_buf()),
            Err(e) => res.failed.push((p.clone(), e)),
        }
    }
    #[cfg(test)]
    let batch: Result<(), String> = ok.iter().try_for_each(|p| fs::remove_file(p).map_err(|e| e.to_string()));
    #[cfg(not(test))]
    let batch: Result<(), String> = trash::delete_all(&ok).map_err(|e| e.to_string());
    match batch {
        Ok(()) => res.deleted.extend(ok.iter().map(|p| p.to_string_lossy().into_owned())),
        Err(_) => {
            // Fall back to one by one, so one locked file doesn't block the rest.
            for p in ok {
                match discard(&p) {
                    Ok(()) => res.deleted.push(p.to_string_lossy().into_owned()),
                    Err(e) => res.failed.push((p.to_string_lossy().into_owned(), friendly_delete_error(&e))),
                }
            }
        }
    }
    res
}

fn friendly_delete_error(e: &str) -> String {
    if e.contains("32") || e.to_lowercase().contains("used by another process") {
        "It's open in another app".into()
    } else {
        e.to_string()
    }
}

/// Put files back from the Recycle Bin (newest deletion of each path).
pub fn restore_clips(paths: &[String]) -> Result<usize, String> {
    let wanted: std::collections::HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let mut items: Vec<trash::TrashItem> = trash::os_limited::list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|i| wanted.contains(&i.original_parent.join(&i.name)))
        .collect();
    // If the same path was deleted twice, restore only the latest.
    items.sort_by(|a, b| b.time_deleted.cmp(&a.time_deleted));
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.original_parent.join(&i.name)));
    let n = items.len();
    if n == 0 {
        return Err("Those clips aren't in the Recycle Bin anymore.".into());
    }
    trash::os_limited::restore_all(items).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Swap an edited file in for the original (original goes to the Recycle Bin).
/// Nothing is removed unless the edit checks out first.
/// A file's dates (modified, created), to put back on an edit of it.
pub fn dates_of(p: &Path) -> Option<(std::time::SystemTime, Option<std::time::SystemTime>)> {
    let m = fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.created().ok()))
}

/// Give an edit its clip's dates, so a trim keeps its place in the library
/// instead of jumping to today. 1 ms later, so players and thumbnails (which
/// go by the date) still see that the file changed.
pub fn set_dates(p: &Path, (modified, created): (std::time::SystemTime, Option<std::time::SystemTime>)) {
    let Ok(f) = fs::OpenOptions::new().write(true).open(p) else { return };
    let mut t = fs::FileTimes::new().set_modified(modified + std::time::Duration::from_millis(1));
    #[cfg(windows)]
    if let Some(c) = created {
        use std::os::windows::fs::FileTimesExt;
        t = t.set_created(c);
    }
    #[cfg(not(windows))]
    let _ = created;
    let _ = f.set_times(t);
}

pub fn replace_original(original: &str, edited: &str, expected_secs: Option<f64>) -> Result<String, String> {
    let orig = Path::new(original);
    let edited_p = Path::new(edited);
    verify_output(edited_p, expected_secs)?;
    let target = orig.with_extension(ext_of(edited_p));
    discard(orig).map_err(|e| format!("Saved the edit as a new file, but couldn't recycle the original: {e}"))?;
    if target.exists() && target != orig {
        discard(&target)?;
    }
    fs::rename(edited_p, &target).map_err(|e| format!("Couldn't move the edit into place: {e}"))?;
    Ok(target.to_string_lossy().into_owned())
}

/// Part of the picture to keep, as fractions of the frame (0..1), e.g. a 9:16
/// slice from the middle of a 16:9 clip for Shorts.
#[derive(serde::Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Crop {
    /// ffmpeg's crop filter for a `width` × `height` video, in even pixels
    /// (encoders need even sizes), kept inside the frame.
    pub fn filter(&self, width: i64, height: i64) -> String {
        let even = |v: f64| ((v.round() as i64) / 2 * 2).max(2);
        let w = even((self.w.clamp(0.02, 1.0) * width as f64).min(width as f64));
        let h = even((self.h.clamp(0.02, 1.0) * height as f64).min(height as f64));
        let x = even(self.x.clamp(0.0, 1.0) * width as f64).min(width - w).max(0);
        let y = even(self.y.clamp(0.0, 1.0) * height as f64).min(height - h).max(0);
        format!("crop={w}:{h}:{x}:{y}")
    }

    /// The whole frame (or near enough): nothing to crop.
    pub fn is_whole(&self) -> bool {
        self.x <= 0.002 && self.y <= 0.002 && self.w >= 0.998 && self.h >= 0.998
    }
}

/// Trim (or keep the whole clip) and crop the picture: the video has to be
/// re-encoded to crop it, in the clip's own format; audio edits apply too.
pub fn trim_cropped(path: &str, start: f64, end: f64, audio: &AudioEdit, crop: Crop) -> Result<String, String> {
    if end <= start {
        return Err("The end point has to be after the start point.".into());
    }
    let p = probe(path)?;
    if p.width <= 0 || p.height <= 0 {
        return Err("Couldn't read the video's size.".into());
    }
    let src = Path::new(path);
    let ext = match ext_of(src).as_str() {
        "mkv" | "mp4" | "webm" | "mov" => ext_of(src),
        _ => "mp4".into(),
    };
    let out = unique(src, " (crop)", &ext);
    let (ss, to) = (format!("{start:.3}"), format!("{end:.3}"));
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-ss", &ss, "-to", &to, "-i"]);
    args.push(path.into());
    let n = p.audio_tracks as usize;
    let mut graph = vec![format!("[0:v:0]{}[v]", crop.filter(p.width, p.height))];
    for i in 0..n {
        // The input starts at the cut (-ss before -i), so mute times shift by `start`.
        graph.push(format!("{}anull[a{i}]", audio.chain(i, start)));
    }
    args.extend(ffmpeg::args(&["-filter_complex", &graph.join(";"), "-map", "[v]"]));
    for i in 0..n {
        args.extend(ffmpeg::args(&["-map", &format!("[a{i}]"), &format!("-map_metadata:s:a:{i}"), &format!("0:s:a:{i}")]));
    }
    let gop = (p.fps.round() as i64).max(1).to_string();
    args.extend(ffmpeg::args(&[
        "-c:v", nvenc_for(&p.codec), "-preset", "p5", "-cq", "20", "-b:v", "0", "-g", &gop, "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k",
    ]));
    if ext == "mp4" {
        args.extend(ffmpeg::args(&["-movflags", "+faststart"]));
    }
    args.push(out.clone().into());
    finish(args, out)
}

/// An H.264 + AAC MP4 that plays everywhere (audio tracks mixed into one).
pub fn export_h264(path: &str, audio: &AudioEdit, crop: Option<Crop>) -> Result<String, String> {
    export_h264_vf(path, unique(Path::new(path), " (H.264)", "mp4"), audio, crop)
}

pub fn export_h264_into(path: &str, out: PathBuf, audio: &AudioEdit) -> Result<String, String> {
    export_h264_vf(path, out, audio, None)
}

fn export_h264_vf(path: &str, out: PathBuf, audio: &AudioEdit, crop: Option<Crop>) -> Result<String, String> {
    let info = probe(path)?;
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-i"]);
    args.push(path.into());
    map_mixed_audio(&mut args, info.audio_tracks, audio);
    if let Some(c) = crop.filter(|c| !c.is_whole()) {
        args.extend(ffmpeg::args(&["-vf", &c.filter(info.width, info.height)]));
    }
    args.extend(ffmpeg::args(&[
        "-c:v", "h264_nvenc", "-preset", "p5", "-cq", "21", "-b:v", "0",
        "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart",
    ]));
    args.push(out.clone().into());
    finish(args, out)
}

/// Just the sound of [start, end] as an MP3: every track mixed into one, or
/// one track on its own (`track`). Audio edits (mutes, volume) are applied.
/// `label` names the file, e.g. "Clip (Microphone).mp3".
pub fn export_audio(path: &str, start: f64, end: f64, track: Option<usize>, label: &str, audio: &AudioEdit) -> Result<String, String> {
    let info = probe(path)?;
    let n = info.audio_tracks as usize;
    if n == 0 {
        return Err("This clip has no audio.".into());
    }
    if track.is_some_and(|i| i >= n) {
        return Err("That audio track isn't in this clip.".into());
    }
    let start = start.max(0.0);
    let end = if end > start + 0.05 { end } else { info.duration };
    let label: String = label.chars().filter(|c| c.is_alphanumeric() || " -_&".contains(*c)).collect();
    let label = label.trim();
    let suffix = if label.is_empty() { " (audio)".to_string() } else { format!(" ({label})") };
    let out = unique(Path::new(path), &suffix, "mp3");
    let (ss, to) = (format!("{start:.3}"), format!("{end:.3}"));
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-ss", &ss, "-to", &to, "-i"]);
    args.push(path.into());
    // The input starts at the cut (-ss before -i), so mute times shift by `start`.
    let graph = match track {
        Some(i) => format!("{}anull[out]", audio.chain(i, start)),
        None if n == 1 => format!("{}anull[out]", audio.chain(0, start)),
        None => {
            let mut parts: Vec<String> = (0..n).map(|i| format!("{}anull[t{i}]", audio.chain(i, start))).collect();
            let inputs: String = (0..n).map(|i| format!("[t{i}]")).collect();
            parts.push(format!("{inputs}amix=inputs={n}:duration=longest:normalize=0[out]"));
            parts.join(";")
        }
    };
    args.extend(ffmpeg::args(&["-filter_complex", &graph, "-map", "[out]", "-vn", "-c:a", "libmp3lame", "-b:a", "256k"]));
    args.push(out.clone().into());
    finish(args, out)
}

/// Fit under `target_mb` (e.g. Discord's limit). Drops to 720p when the bitrate gets low.
pub fn compress(path: &str, target_mb: u32, audio: &AudioEdit, crop: Option<Crop>) -> Result<String, String> {
    compress_vf(path, target_mb, unique(Path::new(path), &format!(" ({target_mb} MB)"), "mp4"), audio, crop)
}

pub fn compress_into(path: &str, target_mb: u32, out: PathBuf, audio: &AudioEdit) -> Result<String, String> {
    compress_vf(path, target_mb, out, audio, None)
}

fn compress_vf(path: &str, target_mb: u32, out: PathBuf, audio: &AudioEdit, crop: Option<Crop>) -> Result<String, String> {
    let info = probe(path)?;
    if info.duration <= 0.0 {
        return Err("Couldn't read the video length.".into());
    }
    let audio_kbps = if info.audio_tracks > 0 { 128.0 } else { 0.0 };
    let total_kbps = target_mb as f64 * 8.0 * 1024.0 * 0.93 / info.duration;
    let video_kbps = (total_kbps - audio_kbps).max(300.0).round() as u64;
    let (b, buf) = (format!("{video_kbps}k"), format!("{}k", video_kbps * 2));
    let mut args = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-i"]);
    args.push(path.into());
    map_mixed_audio(&mut args, info.audio_tracks, audio);
    let mut vf = Vec::new();
    if let Some(c) = crop.filter(|c| !c.is_whole()) {
        vf.push(c.filter(info.width, info.height));
    }
    if video_kbps < 2500 && info.height > 720 {
        vf.push("scale=-2:'min(720,ih)'".to_string());
    }
    if !vf.is_empty() {
        args.extend(ffmpeg::args(&["-vf", &vf.join(",")]));
    }
    args.extend(ffmpeg::args(&[
        "-c:v", "h264_nvenc", "-preset", "p6", "-rc", "vbr", "-b:v", &b, "-maxrate", &b, "-bufsize", &buf,
        "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart",
    ]));
    args.push(out.clone().into());
    finish(args, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6 s 720p60 test video with two audio tracks (like a real Hindsight clip).
    /// A 6 s, 60 fps clip like Hindsight's (keyframe every second, two audio
    /// tracks, video starting one frame in) in the given codec.
    fn rewind_like(dir: &Path, name: &str, vcodec: &str) -> String {
        crate::ffmpeg::init(None);
        fs::create_dir_all(dir).unwrap();
        let out = dir.join(name);
        let mut args = ffmpeg::args(&[
            "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=60",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000",
            "-t", "6", "-map", "0:v", "-map", "1:a", "-map", "2:a",
            "-c:v", vcodec, "-g", "60", "-bf", "0", "-pix_fmt", "yuv420p", "-c:a", "aac",
            "-metadata:s:a:0", "title=Desktop audio", "-metadata:s:a:1", "title=Microphone",
            "-output_ts_offset", "0.017",
        ]);
        args.push(out.clone().into());
        ffmpeg::run("ffmpeg", &args).expect("make sample");
        out.to_string_lossy().into_owned()
    }

    /// MD5 of each decoded frame, `from` frame `skip` on, `n` frames.
    fn frame_hashes(path: &str, skip: usize, n: usize) -> Vec<String> {
        let mut a = ffmpeg::args(&["-v", "error", "-i"]);
        a.push(path.into());
        a.extend(ffmpeg::args(&["-map", "0:v:0", "-vf", &format!("trim=start_frame={skip}:end_frame={}", skip + n), "-f", "framemd5", "-"]));
        ffmpeg::run("ffmpeg", &a).unwrap().lines().filter(|l| !l.starts_with('#')).map(|l| l.rsplit(',').next().unwrap().trim().to_string()).collect()
    }

    #[test]
    fn nas_clips_reuse_what_was_made_for_their_copy_on_this_pc() {
        set_twin("\\\\nas\\clips\\", "C:\\Videos\\Hindsight");
        assert_eq!(local_twin("\\\\NAS\\Clips\\VALORANT\\a.mkv").as_deref(), Some("C:\\Videos\\Hindsight\\VALORANT\\a.mkv"));
        assert_eq!(local_twin("D:\\Medal\\a.mp4"), None);
        set_twin("", "");
        assert_eq!(local_twin("\\\\nas\\clips\\VALORANT\\a.mkv"), None);
    }

    #[test]
    fn crop_is_even_and_inside_the_frame() {
        let c = Crop { x: 0.3, y: 0.0, w: 0.3164, h: 1.0 };
        assert_eq!(c.filter(1920, 1080), "crop=606:1080:576:0");
        // Off the edge: pulled back inside.
        assert_eq!(Crop { x: 0.9, y: 0.9, w: 0.5, h: 0.5 }.filter(1920, 1080), "crop=960:540:960:540");
        assert!(Crop { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }.is_whole());
    }

    #[test]
    fn cropped_trim_has_the_new_size_and_the_audio() {
        let d = tmp("crop");
        let src = rewind_like(&d, "c.mkv", "hevc_nvenc");
        let out = trim_cropped(&src, 1.0, 4.0, &AudioEdit::default(), Crop { x: 0.25, y: 0.0, w: 0.5, h: 1.0 }).unwrap();
        let p = probe(&out).unwrap();
        assert_eq!((p.width, p.height), (640, 720));
        assert!((p.duration - 3.0).abs() < 0.1, "{}", p.duration);
        assert_eq!(p.audio_tracks, 2);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn smart_trim_is_exact_and_copies_untouched_frames() {
        let d = tmp("smart");
        for (codec, name) in [("av1_nvenc", "a.mkv"), ("hevc_nvenc", "h.mkv"), ("h264_nvenc", "x.mp4")] {
            let src = rewind_like(&d, name, codec);
            // Start between keyframes (frame 76 of the clip), end mid-GOP.
            let (start, end) = (0.017 + 76.0 / 60.0 + 0.004, 0.017 + 280.0 / 60.0 + 0.004);
            let t = std::time::Instant::now();
            let out = trim(&src, start, end, true, &AudioEdit::default()).unwrap();
            let took = t.elapsed();
            let p = probe(&out).unwrap();
            let mut a = ffmpeg::args(&["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", "-i"]);
            a.push(out.clone().into());
            let frames: i64 = ffmpeg::run("ffprobe", &a).unwrap().trim().parse().unwrap();
            println!("{codec}: {frames} frames, {:.3} s, {} audio tracks, {took:?}", p.duration, p.audio_tracks);
            assert_eq!(frames, 281 - 76, "{codec}: exact frame count");
            assert_eq!(p.audio_tracks, 2);
            // Frames 120.. of the source (keyframe at 2 s) are copied: identical after decoding.
            let copied = frame_hashes(&out, 120 - 76, 60);
            let original = frame_hashes(&src, 120, 60);
            assert_eq!(copied, original, "{codec}: copied frames must be bit-identical");
        }
        let _ = fs::remove_dir_all(d);
    }

    /// Every AV1 level the stream's sequence headers announce, start to end.
    fn av1_levels(path: &str) -> std::collections::BTreeSet<String> {
        let mut a = ffmpeg::args(&["-hide_banner", "-v", "trace", "-i"]);
        a.push(path.into());
        a.extend(ffmpeg::args(&["-map", "0:v:0", "-c", "copy", "-bsf:v", "trace_headers", "-f", "null", "-"]));
        let out = crate::ffmpeg::cmd("ffmpeg").args(&a).output().unwrap();
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| l.contains("seq_level_idx[0]"))
            .filter_map(|l| l.rsplit('=').next().map(|v| v.trim().to_string()))
            .collect()
    }

    /// Clips from other recorders (OBS, NVIDIA) carry a real level, e.g. 4.1, and full-range colour.
    /// The trimmed file must keep exactly that everywhere, and still copy the untouched frames.
    #[test]
    fn smart_trim_keeps_the_source_level_and_profile() {
        let d = tmp("smart-level");
        crate::ffmpeg::init(None);
        fs::create_dir_all(&d).unwrap();
        for (codec, name, extra) in [
            ("av1_nvenc", "obs.mkv", vec!["-level", "9", "-color_range", "pc"]),
            ("hevc_nvenc", "obs-h.mkv", vec!["-level", "123"]),
            ("h264_nvenc", "obs-x.mkv", vec!["-level", "41", "-profile:v", "main"]),
        ] {
            let src = d.join(name);
            let mut args = ffmpeg::args(&[
                "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=60",
                "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "6", "-map", "0:v", "-map", "1:a",
                "-c:v", codec, "-g", "250", "-bf", "0", "-pix_fmt", "yuv420p",
            ]);
            args.extend(ffmpeg::args(&extra));
            args.extend(ffmpeg::args(&["-c:a", "aac"]));
            args.push(src.clone().into());
            ffmpeg::run("ffmpeg", &args).expect("make sample");
            // Keyframes every 250 frames (like OBS): 0 and 4.17 s. Start mid-GOP.
            let src_s = src.to_string_lossy().into_owned();
            let out = trim(&src_s, 1.5, 5.8, true, &AudioEdit::default()).unwrap();
            let sig = |p: &str| {
                let mut a = ffmpeg::args(&["-v", "error", "-select_streams", "v:0", "-print_format", "json", "-show_streams", "-i"]);
                a.push(p.into());
                let j: Value = serde_json::from_str(&ffmpeg::run("ffprobe", &a).unwrap()).unwrap();
                stream_sig(&j["streams"][0])
            };
            assert_eq!(sig(&out), sig(&src_s), "{codec}: same profile, level and colour as the source");
            if codec == "av1_nvenc" {
                let levels = av1_levels(&out);
                assert_eq!(levels.into_iter().collect::<Vec<_>>(), vec!["9".to_string()], "one AV1 level all the way through");
            }
            // The keyframe at 250 is copied: identical to the source (smart path used, not a full re-encode).
            let at = 250 - 90; // trim starts at frame 90 (1.5 s)
            assert_eq!(frame_hashes(&out, at, 20), frame_hashes(&src_s, 250, 20), "{codec}: copied frames must be bit-identical");
        }
        let _ = fs::remove_dir_all(d);
    }

    /// Smart-trims a real clip: REWIND_TRIM_CLIP=path, cuts 5.237 s to 10 s before the end.
    /// cargo test smart_trim_real_clip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn smart_trim_real_clip() {
        crate::ffmpeg::init(None);
        let Ok(src) = std::env::var("REWIND_TRIM_CLIP") else { return };
        let len = probe(&src).unwrap().duration;
        let t = std::time::Instant::now();
        let out = trim(&src, 5.237, len - 10.0, true, &AudioEdit::default()).unwrap();
        let p = probe(&out).unwrap();
        println!("trimmed {:.1} s of {:.1} s in {:?}: {:.3} s, {} MB, {} audio tracks", len - 15.237, len, t.elapsed(), p.duration, fs::metadata(&out).unwrap().len() / 1_000_000, p.audio_tracks);
        if p.codec == "av1" {
            println!("AV1 levels in the source: {:?}, in the trim: {:?}", av1_levels(&src), av1_levels(&out));
        }
        let _ = fs::remove_file(out);
    }

    fn sample(dir: &Path, name: &str) -> String {
        crate::ffmpeg::init(None);
        fs::create_dir_all(dir).unwrap();
        let out = dir.join(name);
        let mut args = ffmpeg::args(&[
            "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=60",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000",
            "-t", "6", "-map", "0:v", "-map", "1:a", "-map", "2:a",
            "-c:v", "av1_nvenc", "-g", "120", "-c:a", "aac",
        ]);
        args.push(out.clone().into());
        ffmpeg::run("ffmpeg", &args).expect("make sample");
        out.to_string_lossy().into_owned()
    }

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rewind-test-{name}-{}", std::process::id()))
    }

    #[test]
    fn probe_reads_tracks_and_length() {
        let d = tmp("probe");
        let p = probe(&sample(&d, "a.mkv")).unwrap();
        assert_eq!(p.codec, "av1");
        assert_eq!(p.audio_tracks, 2);
        assert!((p.duration - 6.0).abs() < 0.2, "duration {}", p.duration);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn preview_tracks_extracts_the_mic_track() {
        let d = tmp("preview");
        let src = sample(&d, "a.mkv");
        let tracks = preview_tracks(&src, &d.join("cache")).unwrap();
        assert_eq!(tracks.len(), 1, "one extra track besides the first");
        assert_eq!(tracks[0].index, 1);
        let p = probe(&tracks[0].path).unwrap();
        assert_eq!(p.audio_tracks, 1);
        assert!(p.codec.is_empty(), "audio only");
        assert!((p.duration - 6.0).abs() < 0.3, "{}", p.duration);
        // Second call reuses the cached file.
        let again = preview_tracks(&src, &d.join("cache")).unwrap();
        assert_eq!(again[0].path, tracks[0].path);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn trims_fast_and_exact_keep_both_audio_tracks() {
        let d = tmp("trim");
        let src = sample(&d, "a.mkv");
        let fast = trim(&src, 1.0, 4.0, false, &AudioEdit::default()).unwrap();
        let exact = trim(&src, 1.5, 3.5, true, &AudioEdit::default()).unwrap();
        let (f, e) = (probe(&fast).unwrap(), probe(&exact).unwrap());
        assert!((f.duration - 3.0).abs() < 2.1, "fast {}", f.duration); // keyframe-aligned
        assert!((e.duration - 2.0).abs() < 0.15, "exact {}", e.duration);
        assert_eq!((f.audio_tracks, e.audio_tracks), (2, 2));
        assert!(Path::new(&src).exists(), "original must be untouched");
        assert!(trim(&src, 3.0, 2.0, true, &AudioEdit::default()).is_err(), "end before start is refused");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn remux_keeps_everything_and_original() {
        let d = tmp("remux");
        let src = sample(&d, "a.mkv");
        let out = remux_mp4(&src, true).unwrap();
        assert!(out.ends_with("a.mp4"));
        let p = probe(&out).unwrap();
        assert_eq!((p.codec.as_str(), p.audio_tracks), ("av1", 2));
        assert!(Path::new(&src).exists());
        // Remuxing again must not overwrite the first MP4.
        let out2 = remux_mp4(&src, true).unwrap();
        assert_ne!(out, out2);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn remux_without_keep_removes_original_only_after_success() {
        let d = tmp("remux-nokeep");
        let src = sample(&d, "a.mkv");
        let out = remux_mp4(&src, false).unwrap();
        assert!(Path::new(&out).exists());
        assert!(!Path::new(&src).exists());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn export_h264_mixes_audio_into_one_track() {
        let d = tmp("export");
        let out = export_h264(&sample(&d, "a.mkv"), &AudioEdit::default(), None).unwrap();
        let p = probe(&out).unwrap();
        assert_eq!((p.codec.as_str(), p.audio_tracks), ("h264", 1));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn medal_sound_files_are_part_of_their_clip() {
        let d = tmp("sidecar");
        fs::create_dir_all(&d).unwrap();
        for n in ["Clip1.mp4", "Clip1_Game.mp4", "Clip1_Mic.mp4", "Lonely_Game.mp4"] {
            fs::write(d.join(n), b"x").unwrap();
        }
        assert!(is_sidecar(&d.join("Clip1_Game.mp4")) && is_sidecar(&d.join("Clip1_Mic.mp4")));
        assert!(!is_sidecar(&d.join("Clip1.mp4")));
        assert!(!is_sidecar(&d.join("Lonely_Game.mp4")), "no main clip beside it: it's shown");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn export_audio_is_an_mp3_of_the_range() {
        let d = tmp("export-audio");
        let src = sample(&d, "a.mkv");
        let mixed = export_audio(&src, 0.5, 1.5, None, "", &AudioEdit::default()).unwrap();
        assert!(mixed.ends_with("a (audio).mp3"), "{mixed}");
        let p = probe(&mixed).unwrap();
        assert_eq!((p.codec.as_str(), p.audio_tracks), ("", 1));
        assert!((p.duration - 1.0).abs() < 0.15, "{}", p.duration);
        let one = export_audio(&src, 0.0, 0.0, Some(1), "Microphone", &AudioEdit::default()).unwrap();
        assert!(one.ends_with("a (Microphone).mp3"), "{one}");
        assert!(export_audio(&src, 0.0, 1.0, Some(9), "", &AudioEdit::default()).is_err());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn compress_hits_target_size() {
        let d = tmp("compress");
        let out = compress(&sample(&d, "a.mkv"), 1, &AudioEdit::default(), None).unwrap();
        let size = fs::metadata(&out).unwrap().len();
        assert!(size <= 1024 * 1024 + 64 * 1024, "{size} bytes");
        assert_eq!(probe(&out).unwrap().audio_tracks, 1);
        let _ = fs::remove_dir_all(d);
    }

    /// Mean level (0-255) of a track between two times of a file.
    fn level(path: &str, track: u32, a: f64, b: f64) -> f64 {
        let w = crate::audio_edit::decode(path, track).unwrap();
        let (i, j) = ((a / w.step) as usize, (b / w.step) as usize);
        let s = &w.levels[i..j.min(w.levels.len())];
        s.iter().map(|&v| v as f64).sum::<f64>() / s.len().max(1) as f64
    }

    #[test]
    fn trim_mutes_and_boosts_the_chosen_track_only() {
        use crate::audio_edit::TrackEdit;
        let d = tmp("audio-edit");
        let src = sample(&d, "a.mkv");
        // Mute the mic (track 2) from 2.0 to 3.0 s of the original, boost the game track.
        let edit = AudioEdit(vec![
            TrackEdit { gain_db: 6.0, mutes: vec![] },
            TrackEdit { gain_db: 0.0, mutes: vec![[2.0, 3.0]] },
        ]);
        for exact in [true, false] {
            let out = trim(&src, 1.0, 5.0, exact, &edit).unwrap();
            assert_eq!(probe(&out).unwrap().audio_tracks, 2);
            // In the trimmed file the mute sits at 1.0-2.0 s.
            assert!(level(&out, 1, 1.15, 1.85) < 5.0, "muted stretch is silent ({exact})");
            assert!(level(&out, 1, 0.2, 0.8) > 150.0, "mic before the mute is kept ({exact})");
            assert!(level(&out, 1, 2.3, 3.5) > 150.0, "mic after the mute is kept ({exact})");
            assert!(level(&out, 0, 1.15, 1.85) > 150.0, "game track isn't muted ({exact})");
            let louder = level(&out, 0, 0.5, 3.5) - level(&src, 0, 1.5, 4.5);
            assert!(louder > 20.0, "+6 dB shows ({louder}, {exact})");
            let _ = fs::remove_file(out);
        }
        // Export mixes the tracks, with the same edits.
        let out = export_h264(&src, &edit, None).unwrap();
        assert!(level(&out, 0, 2.15, 2.85) > 150.0, "the game track still plays in the mix");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn replace_original_swaps_in_the_edit() {
        let d = tmp("replace");
        let src = sample(&d, "a.mkv");
        let edited = trim(&src, 1.0, 3.0, true, &AudioEdit::default()).unwrap();
        let final_path = replace_original(&src, &edited, Some(2.0)).unwrap();
        assert_eq!(final_path, src, "same name, same place");
        // The edit keeps the clip's date (set by `edit` in lib.rs through these two).
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        set_dates(Path::new(&final_path), (old, Some(old)));
        let (m, c) = dates_of(Path::new(&final_path)).unwrap();
        assert_eq!(m, old + std::time::Duration::from_millis(1));
        assert_eq!(c, Some(old));
        assert!(!Path::new(&edited).exists());
        assert!((probe(&final_path).unwrap().duration - 2.0).abs() < 0.15);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn replace_original_refuses_a_bad_edit() {
        let d = tmp("replace-bad");
        let src = sample(&d, "a.mkv");
        let bad = d.join("broken.mkv");
        fs::write(&bad, b"not a video").unwrap();
        assert!(replace_original(&src, &bad.to_string_lossy(), None).is_err());
        let empty = d.join("empty.mkv");
        fs::write(&empty, b"").unwrap();
        assert!(replace_original(&src, &empty.to_string_lossy(), None).is_err());
        // A 6 s file posing as a 2 s trim is refused too.
        let copy = d.join("copy.mkv");
        fs::copy(&src, &copy).unwrap();
        assert!(replace_original(&src, &copy.to_string_lossy(), Some(2.0)).is_err());
        assert!(Path::new(&src).exists(), "original must survive every refusal");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn delete_only_touches_library_videos() {
        let d = tmp("delete");
        let clips = d.join("clips");
        fs::create_dir_all(clips.join("VALORANT")).unwrap();
        let a = clips.join("VALORANT").join("a.mkv");
        let b = clips.join("VALORANT").join("b.mp4");
        let outside = d.join("outside.mkv");
        let imported = d.join("imported.mp4");
        let not_video = clips.join("notes.txt");
        for f in [&a, &b, &outside, &imported, &not_video] {
            fs::write(f, b"x").unwrap();
        }
        let s = |p: &PathBuf| p.to_string_lossy().into_owned();
        let ext = vec![s(&imported)];
        let r = delete_clips(&[s(&a), s(&b), s(&outside), s(&imported), s(&not_video)], &clips, &ext, &[], "");
        assert_eq!(r.deleted.len(), 3, "{:?}", r.failed);
        assert!(!a.exists() && !b.exists() && !imported.exists());
        assert!(outside.exists(), "files outside the library are never deleted");
        assert!(not_video.exists(), "non-videos are never deleted");
        assert_eq!(r.failed.len(), 2);
        // Already-deleted paths count as done, not as errors.
        let again = delete_clips(&[s(&a)], &clips, &ext, &[], "");
        assert_eq!((again.deleted.len(), again.failed.len()), (1, 0));
        let _ = fs::remove_dir_all(d);
    }

    /// Uses the real Recycle Bin: cargo test recycle_ -- --ignored
    #[test]
    #[ignore]
    fn recycle_and_undo_roundtrip() {
        let d = tmp("recycle");
        fs::create_dir_all(&d).unwrap();
        let f = d.join(format!("rewind-undo-test-{}.mkv", std::process::id()));
        fs::write(&f, b"clip").unwrap();
        let p = f.to_string_lossy().into_owned();
        trash::delete_all([&f]).unwrap();
        assert!(!f.exists());
        assert_eq!(restore_clips(std::slice::from_ref(&p)).unwrap(), 1);
        assert_eq!(fs::read(&f).unwrap(), b"clip");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn scan_skips_hidden_folders_and_groups_by_game() {
        let d = tmp("scan");
        fs::create_dir_all(d.join("VALORANT")).unwrap();
        fs::create_dir_all(d.join(".rewind-recording-x")).unwrap();
        fs::write(d.join("VALORANT").join("clip.mkv"), b"x").unwrap();
        fs::write(d.join(".rewind-recording-x").join("000001_seg.mkv"), b"x").unwrap();
        let clips = scan(&d, &[]);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].folder, "VALORANT");
        let _ = fs::remove_dir_all(d);
    }
}

#[cfg(test)]
mod library_merge_tests {
    use super::*;

    fn put(root: &Path, rel: &str) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        p
    }

    #[test]
    fn one_library_from_this_pc_the_nas_and_other_recorders() {
        let d = std::env::temp_dir().join(format!("rewind-merge-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (clips, nas, medal, obs) = (d.join("Hindsight"), d.join("nas"), d.join("Medal"), d.join("OBS"));
        put(&clips, "VALORANT/a.mkv");
        put(&nas, "VALORANT/a.mkv"); // also on the NAS
        put(&nas, "VALORANT/b - OTHER-PC.mkv"); // only on the NAS
        put(&medal, "Clips/Valorant/m.mp4");
        put(&medal, "Thumbnails/m.mp4"); // not a clip folder
        put(&obs, "2026-11-03 10-00-00.mkv");
        let nas_files: Vec<crate::nas::NasFile> = vec![
            crate::nas::NasFile { path: nas.join("VALORANT/a.mkv"), rel: "VALORANT/a.mkv".into(), folder: "VALORANT".into() },
            crate::nas::NasFile { path: nas.join("VALORANT/b - OTHER-PC.mkv"), rel: "VALORANT/b - OTHER-PC.mkv".into(), folder: "VALORANT".into() },
        ];
        let linked = vec![LinkedRoot { path: medal.clone(), label: "Medal".into() }, LinkedRoot { path: obs.clone(), label: "OBS".into() }];
        let all = scan_all(&clips, &[], &nas_files, &linked, &nas.to_string_lossy());
        let find = |name: &str| all.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(all.len(), 4, "{:?}", all.iter().map(|c| &c.path).collect::<Vec<_>>());
        let a = find("a");
        assert!(a.location == "local" && a.on_nas);
        let b = find("b - OTHER-PC");
        assert!(b.location == "nas" && b.folder == "VALORANT" && b.source == "NAS");
        let m = find("m");
        assert!(m.location == "linked" && m.folder == "Valorant" && m.source == "Medal");
        assert_eq!(find("2026-11-03 10-00-00").folder, "OBS");
        // NAS clips can't be deleted from Hindsight (no Recycle Bin there); linked ones can.
        assert!(deletable(&nas.join("VALORANT/b - OTHER-PC.mkv"), &clips, &[], &[], &nas.to_string_lossy()).is_err());
        assert!(deletable(&medal.join("Clips/Valorant/m.mp4"), &clips, &[], &[medal.to_string_lossy().into_owned()], "").is_ok());
        let _ = fs::remove_dir_all(&d);
    }
}
#[cfg(test)]
mod live_scan {
    /// How long listing a real clips folder takes:
    /// set REWIND_SCAN_DIR, then cargo test --lib live_scan_timing -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_scan_timing() {
        let Ok(dir) = std::env::var("REWIND_SCAN_DIR") else { return };
        for round in 0..3 {
            let t = std::time::Instant::now();
            let clips = super::scan(std::path::Path::new(&dir), &[]);
            println!("round {round}: {} clips in {} ms", clips.len(), t.elapsed().as_millis());
        }
    }
}

#[cfg(test)]
mod live_audio_save {
    /// "Save audio changes" on a copy of a real clip: the whole clip, one muted stretch.
    /// set REWIND_AUDIO_CLIP, then cargo test --lib live_audio_save -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_audio_save() {
        let Ok(src) = std::env::var("REWIND_AUDIO_CLIP") else { return };
        crate::ffmpeg::init(None);
        let dir = std::env::temp_dir().join(format!("hindsight-audio-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join(std::path::Path::new(&src).file_name().unwrap());
        std::fs::copy(&src, &copy).unwrap();
        let path = copy.to_string_lossy().into_owned();
        let p = super::probe(&path).unwrap();
        println!("source: {:.2} s, {} audio tracks, codec {}", p.duration, p.audio_tracks, p.codec);
        let mut edit = crate::audio_edit::AudioEdit::default();
        edit.0 = vec![crate::audio_edit::TrackEdit { gain_db: 0.0, mutes: vec![[5.0, 10.0]] }];
        let t = std::time::Instant::now();
        let out = super::trim(&path, 0.0, p.duration, true, &edit);
        println!("took {:?}: {:?}", t.elapsed(), out);
        if let Ok(o) = &out {
            let q = super::probe(o).unwrap();
            println!("output: {:.2} s, {} audio tracks, {} MB", q.duration, q.audio_tracks, std::fs::metadata(o).unwrap().len() / 1_048_576);
            // Loudness of track 1 inside the muted stretch (6-9 s) and outside it (12-15 s).
            for (from, label) in [(6.0, "muted 6-9 s"), (12.0, "kept 12-15 s")] {
                let mut a = crate::ffmpeg::args(&["-hide_banner", "-ss", &format!("{from}"), "-t", "3", "-i"]);
                a.push(o.into());
                a.extend(crate::ffmpeg::args(&["-map", "0:a:0", "-af", "volumedetect", "-f", "null", "-"]));
                let out = crate::ffmpeg::cmd("ffmpeg").args(&a).output().unwrap();
                let err = String::from_utf8_lossy(&out.stderr);
                println!("  {label}: {}", err.lines().find(|l| l.contains("max_volume")).unwrap_or("?").split("] ").last().unwrap_or(""));
            }
        }
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            println!("  file: {} ({} MB)", e.file_name().to_string_lossy(), e.metadata().unwrap().len() / 1_048_576);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live_twin {
    /// Freed-up clips find the thumbnail made while they were on this PC (read-only).
    /// REWIND_TWIN_LIST: a file of the freed clips' old paths.
    /// cargo test --lib live_twin -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_twin() {
        let home = std::env::var("USERPROFILE").unwrap();
        let clips = format!(r"{home}\Videos\Rewind");
        super::set_twin(r"\\voyager-1\clips", &clips);
        let thumbs = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("com.rewind.app").join("thumbs");
        let list = std::fs::read_to_string(std::env::var("REWIND_TWIN_LIST").unwrap()).unwrap();
        let (mut found, mut total) = (0, 0);
        for old in list.lines().map(|l| l.trim_start_matches('\u{feff}').trim()).filter(|l| !l.is_empty()) {
            let Some(rest) = old.strip_prefix(&format!(r"{clips}\")) else { continue };
            let nas = format!(r"\\voyager-1\clips\{rest}");
            if !std::path::Path::new(&nas).exists() {
                continue;
            }
            total += 1;
            if super::twin_thumbnail(&nas, &thumbs).is_some() {
                found += 1;
            }
        }
        println!("freed clips whose old thumbnail is reused: {found} of {total}");
    }
}
