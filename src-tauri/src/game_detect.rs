//! Which game is this clip? For clips in folders with no game in their name
//! (OBS, Desktop, Imports, a recorder's own folder…), looked up from the
//! picture itself.
//!
//! - An image model (CLIP ViT-B/32, its vision half, quantized ONNX, run by
//!   ONNX Runtime on the CPU) turns a few frames of a clip into a "fingerprint"
//!   of what it looks like: HUD, art style, maps, menus.
//! - Your already-sorted clips are the examples: each unsorted clip gets the
//!   game of the sorted clips it looks most like (a vote among the closest
//!   ones), with a confidence. Nothing is downloaded per game and nothing
//!   leaves the PC.
//! - It only suggests. The app shows the suggestions and you choose what moves.

use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

pub const DIM: usize = 512;
const SIDE: usize = 224;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

pub struct Model {
    session: Session,
}

impl Model {
    /// `runtime` is onnxruntime.dll, `model` the .onnx file.
    pub fn load(runtime: &Path, model: &Path) -> Result<Model, String> {
        ort::init_from(runtime.to_string_lossy().to_string()).commit().map_err(|e| e.to_string())?;
        let session = Session::builder()
            .and_then(|b| b.with_intra_threads(2))
            .and_then(|b| b.commit_from_file(model))
            .map_err(|e| e.to_string())?;
        Ok(Model { session })
    }

    /// One fingerprint per frame (224×224 RGB bytes), each of length 1.
    pub fn embed(&mut self, frames: &[Vec<u8>]) -> Result<Vec<Vec<f32>>, String> {
        if frames.is_empty() {
            return Ok(Vec::new());
        }
        let n = frames.len();
        let mut data = vec![0f32; n * 3 * SIDE * SIDE];
        for (f, rgb) in frames.iter().enumerate() {
            for i in 0..SIDE * SIDE {
                for c in 0..3 {
                    let v = rgb.get(i * 3 + c).copied().unwrap_or(0) as f32 / 255.0;
                    data[f * 3 * SIDE * SIDE + c * SIDE * SIDE + i] = (v - MEAN[c]) / STD[c];
                }
            }
        }
        let input = Tensor::from_array(([n, 3, SIDE, SIDE], data)).map_err(|e| e.to_string())?;
        let outputs = self.session.run(ort::inputs!["pixel_values" => input]).map_err(|e| e.to_string())?;
        let (shape, values) = outputs["image_embeds"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let dim = *shape.last().unwrap_or(&(DIM as i64)) as usize;
        Ok(values.chunks(dim).map(normalized).collect())
    }
}

fn normalized(v: &[f32]) -> Vec<f32> {
    let len = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter().map(|x| x / len).collect()
}

/// `count` frames spread through the clip (skipping its first and last 10%),
/// as 224×224 RGB, cropped to the middle like the model was trained.
pub fn frames(path: &str, duration: f64, count: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for k in 0..count {
        let t = duration * (0.1 + 0.8 * (k as f64 + 0.5) / count as f64);
        let mut c = crate::ffmpeg::cmd("ffmpeg");
        c.args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{t:.2}"), "-i"]).arg(path).args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={SIDE}:{SIDE}:force_original_aspect_ratio=increase,crop={SIDE}:{SIDE}"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ]);
        if let Ok(o) = c.output()
            && o.stdout.len() == SIDE * SIDE * 3
        {
            out.push(o.stdout);
        }
    }
    out
}

/// A clip's fingerprint: the average of its frames', length 1.
pub fn clip_print(model: &mut Model, path: &str, duration: f64) -> Option<Vec<f32>> {
    average(&model.embed(&frames(path, duration, 3)).ok()?)
}

fn average(prints: &[Vec<f32>]) -> Option<Vec<f32>> {
    let mut avg = vec![0f32; prints.first()?.len()];
    for p in prints {
        for (a, x) in avg.iter_mut().zip(p) {
            *a += x;
        }
    }
    Some(normalized(&avg))
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Guess {
    pub game: String,
    /// 0..1: how sure (share of the closest clips' votes, weighted by likeness)
    pub confidence: f32,
    /// How alike the closest example is (cosine, 0..1)
    pub likeness: f32,
}

/// The game of the `k` most alike sorted clips, weighted by how alike they are.
pub fn guess(print: &[f32], examples: &[(String, Vec<f32>)], k: usize) -> Option<Guess> {
    guess_among(print, examples.iter(), k)
}

fn guess_among<'a>(print: &[f32], examples: impl Iterator<Item = &'a (String, Vec<f32>)>, k: usize) -> Option<Guess> {
    let (ranked, likeness) = rank_among(print, examples, k)?;
    let (game, confidence) = ranked.into_iter().next()?;
    Some(Guess { game, confidence, likeness })
}

/// The games of the `k` most alike sorted clips, each with its share of the votes
/// (best first, weighted by how alike they are), and how alike the closest one is.
fn rank_among<'a>(print: &[f32], examples: impl Iterator<Item = &'a (String, Vec<f32>)>, k: usize) -> Option<(Vec<(String, f32)>, f32)> {
    let mut near: Vec<(f32, &str)> = examples.map(|(g, e)| (cosine(print, e), g.as_str())).collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    near.truncate(k);
    let best = near.first()?.0;
    let mut votes: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    for (sim, g) in &near {
        // Sharpen: the closest ones count much more than the k-th.
        *votes.entry(g).or_default() += ((sim - best + 0.08).max(0.0) * 25.0).exp();
    }
    let total: f32 = votes.values().sum::<f32>().max(1e-6);
    let mut ranked: Vec<(String, f32)> = votes.into_iter().map(|(g, v)| (g.to_string(), v / total)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Some((ranked, best))
}

// ---------- in the app: the download, the background job, suggestions ----------

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Where the pack is published, and what each file must hash to.
const PACK: [(&str, &str, u64); 2] = [
    ("onnxruntime.dll", "cb2fa518b18356617f6f9f460c012501da2819ec7e5c1d80bd8f5de20492db0c", 17_253_728),
    ("clip-vision-b32-q.onnx", "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299", 89_117_001),
];
const PACK_URL: &str = "https://github.com/aattiaibrahim/hindsight/releases/download/ai-pack-v1/";

/// Folders that don't say which game a clip is.
pub(crate) const UNSORTED: [&str; 14] = [
    "desktop", "obs", "other", "imports", "imported", "recordings", "screenrecording", "captures", "xboxgamebar", "ascent", "clips", "videos", "temp", "untitled",
];
/// Desktop clips are examples too: of "not a game".
pub const NOT_A_GAME: &str = "Desktop";
/// Sorted clips that are only on the NAS, newest first, looked at per game. Enough
/// examples to recognise it, without reading a whole library over the network.
const NAS_EXAMPLES_PER_GAME: usize = 30;

pub fn is_unsorted(folder: &str, source: &str) -> bool {
    let k = crate::consolidate::game_key(folder);
    k.is_empty() || UNSORTED.contains(&k.as_str()) || (!source.is_empty() && crate::consolidate::game_key(source) == k)
}

/// Your own work, not a game: "Edits", "YT Edit", "YouTube Videos", "render", "exports".
/// They hold clips of many games, so they'd teach the finder nothing and would be
/// suggested as a game's folder.
fn is_workshop(folder: &str) -> bool {
    const WORDS: [&str; 8] = ["edit", "render", "export", "youtube", "montage", "funny", "meme", "highlight"];
    folder
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| WORDS.iter().any(|p| w.starts_with(p)))
}

/// Not a game: your own work ("Edits"), or a folder you marked as not a game
/// (`skip`). Never learned from, never offered as the answer.
pub fn not_a_game_folder(folder: &str, skip: &[String]) -> bool {
    let key = crate::consolidate::game_key(folder);
    is_workshop(folder) || skip.iter().any(|s| crate::consolidate::game_key(s) == key)
}

/// Clips to find the game for: unsorted ones, a folder you asked the finder to look
/// through, and anything in a folder that isn't a game (your own like "aim", and your
/// edit folders): those clips belong in their game's folder too.
fn needs_game(c: &crate::library::ClipFile, also: &[String], skip: &[String]) -> bool {
    let key = crate::consolidate::game_key(&c.folder);
    is_unsorted(&c.folder, &c.source) || not_a_game_folder(&c.folder, skip) || also.iter().any(|f| crate::consolidate::game_key(f) == key)
}

/// The clips worth fingerprinting, examples first so suggestions can start early.
/// `clips` is newest first.
fn to_look_at<'a>(clips: &'a [crate::library::ClipFile], also: &[String], skip: &[String]) -> Vec<&'a crate::library::ClipFile> {
    let mut per_game: HashMap<String, usize> = HashMap::new();
    let (mut examples, mut unsorted) = (Vec::new(), Vec::new());
    for c in clips {
        if c.location != "nas" && c.path.starts_with(r"\\") {
            continue;
        }
        if needs_game(c, also, skip) {
            unsorted.push(c);
            continue;
        }
        if c.location == "nas" {
            let seen = per_game.entry(crate::consolidate::game_key(&c.folder)).or_default();
            *seen += 1;
            if *seen > NAS_EXAMPLES_PER_GAME {
                continue;
            }
        }
        examples.push(c);
    }
    examples.extend(unsorted);
    examples
}

struct State {
    dir: PathBuf,
    model: Option<Model>,
    /// path → (size, modified, fingerprint)
    prints: HashMap<String, (u64, i64, Vec<f32>)>,
    dirty: bool,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub installed: bool,
    /// 0..1 while downloading
    pub downloading: Option<f64>,
    pub looked_at: usize,
    pub to_look_at: usize,
    pub error: Option<String>,
}
static STATUS: Mutex<Option<Status>> = Mutex::new(None);

pub fn status() -> Status {
    let mut s = STATUS.lock().unwrap().clone().unwrap_or_default();
    s.installed = installed();
    s
}

pub fn init(dir: PathBuf) {
    let prints = std::fs::read_to_string(dir.join("prints.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    *STATE.lock().unwrap() = Some(State { dir, model: None, prints, dirty: false });
}

fn dir() -> Option<PathBuf> {
    STATE.lock().unwrap().as_ref().map(|s| s.dir.clone())
}

pub fn installed() -> bool {
    dir().is_some_and(|d| PACK.iter().all(|(n, _, size)| std::fs::metadata(d.join(n)).is_ok_and(|m| m.len() == *size)))
}

fn sha256_file(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Where a download stands: "downloading" (bytes), "checking", "installing", "ready".
#[derive(serde::Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub stage: &'static str,
    pub done: u64,
    pub total: u64,
}

/// Download the pack (about 106 MB) once, checking each file's SHA-256, then
/// load the model once to make sure it works on this PC.
pub fn install(mut progress: impl FnMut(Step)) -> Result<(), String> {
    use std::io::{Read, Write};
    let d = dir().ok_or("not ready")?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let total: u64 = PACK.iter().map(|p| p.2).sum();
    let mut done = 0u64;
    // HTTPS needs its crypto chosen first (as for uploads); doing it twice is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(600)).build().map_err(|e| e.to_string())?;
    progress(Step { stage: "downloading", done: 0, total });
    for (name, hash, size) in PACK {
        let out = d.join(name);
        if std::fs::metadata(&out).is_ok_and(|m| m.len() == size) && sha256_file(&out).as_deref() == Some(hash) {
            done += size;
            progress(Step { stage: "downloading", done, total });
            continue;
        }
        let tmp = d.join(format!("{name}.part"));
        let mut res = client.get(format!("{PACK_URL}{name}")).send().map_err(|e| format!("Couldn't download {name}: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("Couldn't download {name}: {}", res.status()));
        }
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = res.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(Step { stage: "downloading", done, total });
        }
        drop(f);
        progress(Step { stage: "checking", done, total });
        if sha256_file(&tmp).as_deref() != Some(hash) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{name} didn't download correctly (its checksum is wrong). Try again."));
        }
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    }
    progress(Step { stage: "installing", done: total, total });
    {
        let mut g = STATE.lock().unwrap();
        if let Some(state) = g.as_mut()
            && state.model.is_none()
        {
            state.model = Some(Model::load(&state.dir.join(PACK[0].0), &state.dir.join(PACK[1].0)).map_err(|e| format!("The download worked, but the model wouldn't start on this PC: {e}"))?);
        }
    }
    progress(Step { stage: "ready", done: total, total });
    Ok(())
}

/// The fingerprint of a clip, from the cache or by looking at it now. Frames are read
/// before taking the lock: on the NAS that can be slow, and suggestions need the cache.
fn print_of(c: &crate::library::ClipFile) -> Option<Vec<f32>> {
    if let Some(p) = cached(c) {
        return Some(p);
    }
    let duration = crate::prepare::probe(&c.path).ok()?.duration;
    let shots = frames(&c.path, duration, 3);
    let mut g = STATE.lock().unwrap();
    let state = g.as_mut()?;
    if state.model.is_none() {
        state.model = Some(Model::load(&state.dir.join(PACK[0].0), &state.dir.join(PACK[1].0)).ok()?);
    }
    let p = average(&state.model.as_mut()?.embed(&shots).ok()?)?;
    state.prints.insert(c.path.clone(), (c.size, c.modified, p.clone()));
    state.dirty = true;
    Some(p)
}

fn flush() {
    let mut g = STATE.lock().unwrap();
    if let Some(s) = g.as_mut()
        && s.dirty
        && let Ok(t) = serde_json::to_string(&s.prints)
    {
        let _ = std::fs::write(s.dir.join("prints.json"), t);
        s.dirty = false;
    }
}

/// By the size and date the library scan just read, so a big NAS library isn't asked again.
fn cached(c: &crate::library::ClipFile) -> Option<Vec<f32>> {
    let g = STATE.lock().unwrap();
    g.as_ref()?.prints.get(&c.path).filter(|(s, m, _)| (*s, *m) == (c.size, c.modified)).map(|(_, _, p)| p.clone())
}

/// Background job: fingerprint clips on this PC and the NAS (sorted ones are the
/// examples, unsorted ones get suggestions), one at a time, only while no game runs.
/// `clips` gives the library, the extra folders to look through, and the folders
/// that aren't games.
pub fn start(
    clips: impl Fn() -> (Vec<crate::library::ClipFile>, Vec<String>, Vec<String>) + Send + 'static,
    enabled: impl Fn() -> bool + Send + 'static,
    busy: impl Fn() -> bool + Send + 'static,
    changed: impl Fn() + Send + 'static,
) {
    std::thread::Builder::new()
        .name("rewind-detect".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            // Files that are there but can't be read: not tried again every round.
            let mut unreadable = std::collections::HashSet::new();
            loop {
                if !enabled() || !installed() || busy() {
                    flush();
                    std::thread::sleep(Duration::from_secs(20));
                    continue;
                }
                let (library, also, skip) = clips();
                let all = to_look_at(&library, &also, &skip);
                let todo: Vec<_> = all.iter().copied().filter(|c| !unreadable.contains(&c.path) && cached(c).is_none()).collect();
                let looked = all.len() - todo.len();
                *STATUS.lock().unwrap() = Some(Status { looked_at: looked, to_look_at: all.len(), ..Default::default() });
                if todo.is_empty() {
                    flush();
                    std::thread::sleep(Duration::from_secs(120));
                    continue;
                }
                let mut n = 0;
                let mut nas_gone = false;
                for c in todo {
                    if !enabled() || busy() {
                        break;
                    }
                    if print_of(c).is_none() {
                        if Path::new(&c.path).is_file() {
                            unreadable.insert(c.path.clone());
                        } else if c.location == "nas" {
                            nas_gone = true;
                            break;
                        }
                    }
                    n += 1;
                    if n % 50 == 0 {
                        flush();
                        changed();
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                flush();
                changed();
                if nas_gone {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
        })
        .expect("detect thread");
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub game: String,
    pub confidence: f32,
    /// How alike the closest sorted clip is (0..1)
    pub likeness: f32,
    /// Sure enough to move (ticked in the review). Otherwise a close call: shown
    /// unticked, with `choices` to pick from.
    pub sure: bool,
    /// The likeliest games, best first (`game` is the first), each with its share of the votes.
    pub choices: Vec<(String, f32)>,
}

/// Sure enough to tick: most of the votes, and very like a sorted clip.
const SURE_SHARE: f32 = 0.7;
const SURE_LIKENESS: f32 = 0.9;
/// A close call worth showing: a real lead, and like a sorted clip.
const CLOSE_SHARE: f32 = 0.3;
const CLOSE_LIKENESS: f32 = 0.88;

/// Games that replaced another and look alike on screen: (old, new, the day the new
/// one came out, in ms). A clip from before that day is the old game, from after it
/// the new one, whatever the picture says. Game keys, any spelling.
const SUCCESSORS: [(&[&str], &[&str], i64); 1] = [
    // Counter-Strike 2 replaced CS:GO on 27 September 2023.
    (&["counterstrikeglobaloffensive", "csgo"], &["counterstrike2", "cs2"], 1_695_772_800_000),
];

/// `ranked` without a game the clip can't be from given its date (see SUCCESSORS),
/// the others' shares scaled back up to 1.
fn by_date(ranked: Vec<(String, f32)>, modified: i64) -> Vec<(String, f32)> {
    if modified <= 0 {
        return ranked;
    }
    let impossible = |g: &str| {
        let k = crate::consolidate::game_key(g);
        SUCCESSORS.iter().any(|(old, new, day)| {
            let has = |list: &[&str]| ranked.iter().any(|(r, _)| list.contains(&crate::consolidate::game_key(r).as_str()));
            // Only when both are in the running: a lone guess is left to the picture.
            has(old) && has(new) && ((modified >= *day && old.contains(&k.as_str())) || (modified < *day && new.contains(&k.as_str())))
        })
    };
    let kept: Vec<(String, f32)> = ranked.iter().filter(|(g, _)| !impossible(g)).cloned().collect();
    let total: f32 = kept.iter().map(|(_, s)| s).sum();
    if kept.is_empty() || total <= 0.0 {
        return ranked;
    }
    kept.into_iter().map(|(g, s)| (g, s / total)).collect()
}

/// Guesses for clips in unsorted folders (already looked at): sure ones and close calls.
/// `also`: your own folders to look through too ("Find the game" on a folder).
/// `skip`: folders that aren't games (never learned from, never an answer).
pub fn suggestions(clips: &[crate::library::ClipFile], dismissed: &[String], also: &[String], skip: &[String]) -> Vec<Suggestion> {
    suggest_from(clips, dismissed, also, skip, cached)
}

fn suggest_from(
    clips: &[crate::library::ClipFile],
    dismissed: &[String],
    also: &[String],
    skip: &[String],
    print: impl Fn(&crate::library::ClipFile) -> Option<Vec<f32>>,
) -> Vec<Suggestion> {
    rank_clips(clips, dismissed, also, skip, |c| needs_game(c, also, skip), print)
}

/// What checking a game's folder found: its clips that look like another game.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    pub suggestions: Vec<Suggestion>,
    /// Clips in the folder with a fingerprint (the ones the check could judge)
    pub looked_at: usize,
    pub clips: usize,
}

fn in_folder(c: &crate::library::ClipFile, folder: &str) -> bool {
    crate::consolidate::game_key(&c.folder) == crate::consolidate::game_key(folder)
}

/// A game's folder, checked for clips of other games, with the fingerprints already
/// made (instant). Nothing is saved: it's a look, not a setting.
pub fn check_folder(clips: &[crate::library::ClipFile], folder: &str, dismissed: &[String], skip: &[String]) -> FolderCheck {
    let mine: Vec<&crate::library::ClipFile> = clips.iter().filter(|c| in_folder(c, folder)).collect();
    let looked_at = mine.iter().filter(|c| cached(c).is_some()).count();
    let suggestions = rank_clips(clips, dismissed, &[], skip, |c| in_folder(c, folder), cached);
    FolderCheck { suggestions, looked_at, clips: mine.len() }
}

/// `check_folder` after fingerprinting every clip in the folder that hasn't been
/// yet (one at a time, `progress(done, total)`), until `cancel` is set. The
/// fingerprints are kept, so the next quick look sees them too.
pub fn scan_folder(
    clips: &[crate::library::ClipFile],
    folder: &str,
    dismissed: &[String],
    skip: &[String],
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize),
) -> Result<FolderCheck, String> {
    if !installed() {
        return Err("Turn on Find the game in unsorted clips in Settings → Library first (it downloads the model once).".into());
    }
    let todo: Vec<&crate::library::ClipFile> = clips.iter().filter(|c| in_folder(c, folder) && cached(c).is_none()).collect();
    for (i, c) in todo.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        progress(i, todo.len());
        let _ = print_of(c);
        if i % 25 == 24 {
            flush();
        }
    }
    progress(todo.len(), todo.len());
    flush();
    Ok(check_folder(clips, folder, dismissed, skip))
}

/// Guesses for the clips `is_candidate` picks, against the sorted clips (game folders,
/// and Desktop as "not a game"). A clip whose likeliest game is its own folder's is left out.
fn rank_clips(
    clips: &[crate::library::ClipFile],
    dismissed: &[String],
    also: &[String],
    skip: &[String],
    is_candidate: impl Fn(&crate::library::ClipFile) -> bool,
    print: impl Fn(&crate::library::ClipFile) -> Option<Vec<f32>>,
) -> Vec<Suggestion> {
    let mut examples: Vec<(String, Vec<f32>)> = Vec::new();
    for c in clips {
        let unsorted = needs_game(c, also, skip);
        let desktop = crate::consolidate::game_key(&c.folder) == "desktop";
        // Examples: sorted game folders, and Desktop (of "not a game").
        if unsorted && !desktop {
            continue;
        }
        if let Some(p) = print(c) {
            examples.push((if desktop { NOT_A_GAME.to_string() } else { c.folder.clone() }, p));
        }
    }
    // Merge spellings: the most common one names the game.
    let mut names: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (g, _) in &examples {
        *names.entry(crate::consolidate::game_key(g)).or_default().entry(g.clone()).or_default() += 1;
    }
    let label = |g: &str| {
        names.get(&crate::consolidate::game_key(g)).and_then(|m| m.iter().max_by_key(|(_, n)| **n).map(|(s, _)| s.clone())).unwrap_or_else(|| g.to_string())
    };
    let examples: Vec<(String, Vec<f32>)> = examples.into_iter().map(|(g, p)| (label(&g), p)).collect();
    if examples.len() < 5 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for c in clips {
        if !is_candidate(c) || dismissed.contains(&c.path) {
            continue;
        }
        let Some(p) = print(c) else { continue };
        let Some((ranked, likeness)) = rank_among(&p, examples.iter().filter(|(_, e)| e != &p), 5) else { continue };
        let ranked = by_date(ranked, c.modified);
        let Some((game, share)) = ranked.first().cloned() else { continue };
        let here = crate::consolidate::game_key(&c.folder);
        if game == NOT_A_GAME || crate::consolidate::game_key(&game) == here {
            continue;
        }
        // A game with no sorted clips yet stays below the likeness bar.
        let sure = share >= SURE_SHARE && likeness >= SURE_LIKENESS;
        if !sure && (share < CLOSE_SHARE || likeness < CLOSE_LIKENESS) {
            continue;
        }
        let choices: Vec<(String, f32)> = ranked
            .into_iter()
            .filter(|(g, s)| g != NOT_A_GAME && *s >= 0.15 && crate::consolidate::game_key(g) != here)
            .take(3)
            .collect();
        out.push(Suggestion { path: c.path.clone(), name: c.name.clone(), folder: c.folder.clone(), game, confidence: share, likeness, sure, choices });
    }
    out.sort_by(|a, b| b.sure.cmp(&a.sure).then(a.game.cmp(&b.game)).then(a.name.cmp(&b.name)));
    out
}

/// One clip being sorted: its file on this PC, on the NAS, or both.
pub struct SortedClip {
    pub from: Vec<PathBuf>,
    /// `Game/clip.mp4` before and after, for the NAS sync's records
    pub old_rel: Option<String>,
    pub new_rel: String,
}

pub struct SortPlan {
    pub plan: crate::consolidate::Plan,
    pub clips: Vec<SortedClip>,
}

/// Rename one clip to `stem` (its extension stays), on this PC and its NAS copy alike,
/// in the folder it's in. Never overwrites: a name that's taken is refused.
pub fn rename_plan(clips_dir: &Path, nas_dir: &Path, path: &str, stem: &str) -> Result<SortPlan, String> {
    use crate::library::same_path;
    let from = PathBuf::from(path);
    let size = std::fs::metadata(&from).map_err(|_| "That clip isn't there anymore.".to_string())?.len();
    let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = crate::consolidate::safe_name(stem);
    let name = if ext.is_empty() { stem.clone() } else { format!("{stem}.{ext}") };
    if from.file_name().is_some_and(|n| n.to_string_lossy() == name) {
        return Err("That's already its name.".into());
    }
    let mut roots = vec![clips_dir];
    if !nas_dir.as_os_str().is_empty() {
        roots.push(nas_dir);
    }
    let old_rel = roots.iter().find_map(|r| crate::nas::rel_on_nas(&r.to_string_lossy(), path));
    // The same clip in the other place (this PC / the NAS), if it's there: renamed too.
    let mut files: Vec<PathBuf> = vec![from.clone()];
    if let Some(rel) = &old_rel {
        for root in &roots {
            let p = root.join(rel.replace('/', "\\"));
            if !same_path(&p, &from) && std::fs::metadata(&p).is_ok_and(|m| m.is_file() && m.len() == size) {
                files.push(p);
            }
        }
    }
    let mut plan = crate::consolidate::Plan { label: "Renamed".into(), ..Default::default() };
    for p in &files {
        let to = p.with_file_name(&name);
        // A different file with that name (changing only capitals is the same file).
        if to.exists() && !same_path(&to, p) {
            return Err(format!("There's already a clip called “{name}” in that folder."));
        }
        plan.count += 1;
        plan.bytes += size;
        plan.moves.push(crate::consolidate::Move { from: p.clone(), to, size, copied: false });
    }
    let new_rel = match &old_rel {
        Some(rel) => match rel.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{name}"),
            None => name.clone(),
        },
        None => name.clone(),
    };
    Ok(SortPlan { plan, clips: vec![SortedClip { from: files, old_rel, new_rel }] })
}

/// The moves that put each chosen clip in its game's folder, never overwriting. A clip
/// on this PC and the NAS moves on both, under one new name, so sync still sees one
/// clip. A clip only on the NAS is renamed there, not copied back to this PC. A file
/// with the same place but another size is a different clip and stays put.
pub fn plan(clips_dir: &Path, nas_dir: &Path, picks: &[(String, String)]) -> SortPlan {
    use crate::library::same_path;
    let mut plan = crate::consolidate::Plan { label: "Sorted by game".into(), ..Default::default() };
    let mut clips = Vec::new();
    let mut taken = std::collections::HashSet::new();
    let mut roots = vec![clips_dir];
    if !nas_dir.as_os_str().is_empty() {
        roots.push(nas_dir);
    }
    for (path, game) in picks {
        let from = PathBuf::from(path);
        let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let folder = crate::consolidate::safe_name(game);
        let size = std::fs::metadata(&from).map(|m| m.len()).unwrap_or(0);
        let old_rel = roots.iter().find_map(|r| crate::nas::rel_on_nas(&r.to_string_lossy(), path));
        let mut files: Vec<(PathBuf, &Path)> = Vec::new();
        match &old_rel {
            Some(rel) => {
                for root in &roots {
                    let p = root.join(rel.replace('/', "\\"));
                    if same_path(&p, &from) || std::fs::metadata(&p).is_ok_and(|m| m.is_file() && m.len() == size) {
                        files.push((p, root));
                    }
                }
            }
            None => files.push((from.clone(), clips_dir)),
        }
        if files.iter().any(|(p, root)| p.parent().is_some_and(|d| same_path(d, &root.join(&folder)))) {
            continue; // already in that game's folder
        }
        let stem = from.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let mut file = name;
        let mut n = 2;
        while roots.iter().any(|r| {
            let to = r.join(&folder).join(&file);
            to.exists() || taken.contains(&to.to_string_lossy().to_lowercase())
        }) {
            file = format!("{stem} ({n}).{ext}");
            n += 1;
        }
        for r in &roots {
            taken.insert(r.join(&folder).join(&file).to_string_lossy().to_lowercase());
        }
        for (src, root) in &files {
            plan.count += 1;
            plan.bytes += size;
            plan.moves.push(crate::consolidate::Move { from: src.clone(), to: root.join(&folder).join(&file), size, copied: false });
        }
        clips.push(SortedClip { from: files.into_iter().map(|(p, _)| p).collect(), old_rel, new_rel: format!("{folder}/{file}") });
    }
    SortPlan { plan, clips }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec<f32> {
        normalized(&[x, y])
    }

    fn clip(path: &str, folder: &str, location: &'static str) -> crate::library::ClipFile {
        crate::library::ClipFile {
            path: path.into(),
            name: path.into(),
            folder: folder.into(),
            size: 1,
            modified: 1,
            external: false,
            location,
            on_nas: false,
            source: if location == "nas" { "NAS".into() } else { String::new() },
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hindsight-sort-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path, bytes: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn clips_only_on_the_nas_are_examples_and_get_suggestions() {
        let mut all: Vec<_> = (0..5).map(|i| clip(&format!(r"\\nas\c\Apex\{i}.mkv"), "Apex Legends", "nas")).collect();
        all.push(clip(r"\\nas\c\OBS\new.mkv", "OBS", "nas"));
        let got = suggest_from(&all, &[], &[], &[], |c| Some(if c.folder == "OBS" { v(1.0, 0.02) } else { v(1.0, 0.01 + c.path.len() as f32 / 1000.0) }));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].path.as_str(), got[0].game.as_str()), (r"\\nas\c\OBS\new.mkv", "Apex Legends"));
    }

    #[test]
    fn checking_a_game_folder_finds_only_the_clips_of_another_game() {
        let mut all: Vec<_> = (0..5).map(|i| clip(&format!(r"C:\c\Valorant\{i}.mkv"), "Valorant", "local")).collect();
        all.extend((0..5).map(|i| clip(&format!(r"C:\c\Tarkov\{i}.mkv"), "Escape From Tarkov", "local")));
        all.push(clip(r"C:\c\Valorant\oops.mkv", "Valorant", "local"));
        // Valorant clips point one way, Tarkov's the other; "oops" looks like Tarkov.
        let print = |c: &crate::library::ClipFile| {
            let tarkov = c.folder == "Escape From Tarkov" || c.path.ends_with("oops.mkv");
            let n = c.path.len() as f32 / 1000.0;
            Some(if tarkov { v(1.0, 0.01 + n) } else { v(0.01 + n, 1.0) })
        };
        let got = rank_clips(&all, &[], &[], &[], |c| in_folder(c, "valorant"), print);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!((got[0].path.as_str(), got[0].game.as_str()), (r"C:\c\Valorant\oops.mkv", "Escape From Tarkov"));
        // Tarkov's own clips aren't checked when Valorant is.
        assert!(got.iter().all(|s| s.folder == "Valorant"));
        // Turned down before: not shown again.
        assert!(rank_clips(&all, &[r"C:\c\Valorant\oops.mkv".into()], &[], &[], |c| in_folder(c, "Valorant"), print).is_empty());
    }

    #[test]
    fn edit_and_export_folders_are_never_a_game() {
        for f in ["Edits", "YT Edit", "Edit (converted)_AME", "YouTube Videos", "render", "exports"] {
            assert!(is_workshop(f), "{f}");
        }
        for f in ["Valorant", "Surrender", "Credit Card Game", "Apex Legends"] {
            assert!(!is_workshop(f), "{f}");
        }
        let mut all: Vec<_> = (0..5).map(|i| clip(&format!(r"\\nas\c\Edits\{i}.mkv"), "Edits", "nas")).collect();
        all.push(clip(r"\\nas\c\OBS\new.mkv", "OBS", "nas"));
        assert!(suggest_from(&all, &[], &[], &[], |c| Some(v(1.0, 0.01 + c.path.len() as f32 / 1000.0))).is_empty(), "never an answer");
        assert_eq!(to_look_at(&all, &[], &[]).len(), 6, "their clips are read, to be sorted into their games");
        assert!(is_workshop("funnysiegevideo") && is_workshop("funny not cool"));
    }

    #[test]
    fn folders_marked_not_a_game_are_never_an_answer_and_their_clips_get_sorted() {
        let mut all: Vec<_> = (0..5).map(|i| clip(&format!(r"\\nas\c\aim\{i}.mkv"), "aim", "nas")).collect();
        all.extend((0..5).map(|i| clip(&format!(r"\\nas\c\Apex\{i}.mkv"), "Apex Legends", "nas")));
        all.push(clip(r"\\nas\c\OBS\new.mkv", "OBS", "nas"));
        // The OBS clip is closest to the "aim" clips.
        let print = |c: &crate::library::ClipFile| Some(match c.folder.as_str() { "OBS" => v(1.0, 0.02), "aim" => v(1.0, 0.021), _ => v(1.0, 0.3) });
        let got = suggest_from(&all, &[], &[], &[], print);
        assert_eq!(got[0].game, "aim", "unmarked, it's taken for a game");
        let got = suggest_from(&all, &[], &[], &["AIM".into()], print);
        assert!(got.iter().all(|s| s.game != "aim" && s.choices.iter().all(|(g, _)| g != "aim")), "{got:?}");
        // The clips in "aim" move out into their game's folder too.
        assert_eq!(got.iter().filter(|s| s.folder == "aim" && s.game == "Apex Legends").count(), 5, "{got:?}");
        assert_eq!(to_look_at(&all, &[], &["aim".into()]).iter().filter(|c| c.folder == "aim").count(), 5);
    }

    #[test]
    fn cs_go_or_cs2_is_decided_by_the_clips_date() {
        let ranked = || vec![("Counter-strike 2".to_string(), 0.6), ("Counter-strike Global Offensive".to_string(), 0.3), ("Valorant".to_string(), 0.1)];
        let before = 1_660_000_000_000; // August 2022
        let after = 1_760_000_000_000; // October 2025
        let old = by_date(ranked(), before);
        assert_eq!(old[0].0, "Counter-strike Global Offensive");
        assert!(old.iter().all(|(g, _)| g != "Counter-strike 2"));
        assert!((old.iter().map(|(_, s)| s).sum::<f32>() - 1.0).abs() < 1e-4);
        let new = by_date(ranked(), after);
        assert_eq!(new.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(), ["Counter-strike 2", "Valorant"]);
        // A lone guess is left alone, and so is a clip with no date.
        let lone = vec![("Counter-strike 2".to_string(), 1.0)];
        assert_eq!(by_date(lone.clone(), before), lone);
        assert_eq!(by_date(ranked(), 0), ranked());
    }

    #[test]
    fn close_calls_come_unticked_with_their_likely_games() {
        // Three Apex clips and two Valorant clips, all about as close to the new one.
        let mut all: Vec<_> = (0..3).map(|i| clip(&format!(r"\\nas\c\Apex\{i}.mkv"), "Apex Legends", "nas")).collect();
        all.extend((0..2).map(|i| clip(&format!(r"\\nas\c\Valorant\{i}.mkv"), "Valorant", "nas")));
        all.push(clip(r"\\nas\c\OBS\new.mkv", "OBS", "nas"));
        let got = suggest_from(&all, &[], &[], &[], |c| Some(match c.folder.as_str() { "OBS" => v(1.0, 0.0), "Apex Legends" => v(1.0, 0.01 + c.path.len() as f32 / 10000.0), _ => v(1.0, 0.012 + c.path.len() as f32 / 10000.0) }));
        assert_eq!(got.len(), 1, "{got:?}");
        let s = &got[0];
        assert!(!s.sure, "{s:?}");
        assert_eq!(s.choices.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(), ["Apex Legends", "Valorant"]);
        assert_eq!(s.game, "Apex Legends");
    }

    #[test]
    fn the_nas_is_sampled_for_examples_but_every_unsorted_clip_is_read() {
        let mut all: Vec<_> = (0..NAS_EXAMPLES_PER_GAME + 5).map(|i| clip(&format!(r"\\nas\c\Apex\{i}.mkv"), "Apex", "nas")).collect();
        all.extend((0..40).map(|i| clip(&format!(r"\\nas\c\OBS\{i}.mkv"), "OBS", "nas")));
        let list = to_look_at(&all, &[], &[]);
        assert_eq!(list.iter().filter(|c| c.folder == "Apex").count(), NAS_EXAMPLES_PER_GAME);
        assert_eq!(list.iter().filter(|c| c.folder == "OBS").count(), 40);
        assert_eq!(list[0].folder, "Apex", "examples come first");
    }

    // Windows paths (drive letters, backslashes); Linux and macOS: phase 2 of docs/platforms-plan.md.
    #[cfg(windows)]
    #[test]
    fn sorting_moves_both_copies_under_one_free_name() {
        let d = temp("both");
        let (pc, nas) = (d.join("pc"), d.join("nas"));
        put(&pc.join(r"OBS\a.mkv"), "same");
        put(&nas.join(r"OBS\a.mkv"), "same");
        put(&nas.join(r"Apex\a.mkv"), "someone else's clip");
        let p = plan(&pc, &nas, &[(pc.join(r"OBS\a.mkv").to_string_lossy().into_owned(), "Apex".into())]);
        let to: Vec<_> = p.plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(to, vec![pc.join(r"Apex\a (2).mkv"), nas.join(r"Apex\a (2).mkv")]);
        assert_eq!(p.clips[0].old_rel.as_deref(), Some("OBS/a.mkv"));
        assert_eq!(p.clips[0].new_rel, "Apex/a (2).mkv");
        let _ = std::fs::remove_dir_all(d);
    }

    // Windows paths (drive letters, backslashes); Linux and macOS: phase 2 of docs/platforms-plan.md.
    #[cfg(windows)]
    #[test]
    fn renaming_renames_both_copies_and_never_overwrites() {
        let d = temp("rename");
        let (pc, nas) = (d.join("pc"), d.join("nas"));
        put(&pc.join(r"Apex\a.mkv"), "same");
        put(&nas.join(r"Apex\a.mkv"), "same");
        put(&pc.join(r"Apex\taken.mkv"), "another clip");
        let from = pc.join(r"Apex\a.mkv").to_string_lossy().into_owned();
        let p = rename_plan(&pc, &nas, &from, "  my: best clip ").unwrap();
        let to: Vec<_> = p.plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(to, vec![pc.join(r"Apex\my best clip.mkv"), nas.join(r"Apex\my best clip.mkv")]);
        assert_eq!((p.clips[0].old_rel.as_deref(), p.clips[0].new_rel.as_str()), (Some("Apex/a.mkv"), "Apex/my best clip.mkv"));
        assert!(rename_plan(&pc, &nas, &from, "taken").err().is_some_and(|e| e.contains("already a clip called")));
        assert!(rename_plan(&pc, &nas, &from, "a").is_err(), "same name");
        assert!(rename_plan(&pc, &nas, &from, "A").is_ok(), "only capitals changed");
        let _ = std::fs::remove_dir_all(d);
    }

    // Windows paths (drive letters, backslashes); Linux and macOS: phase 2 of docs/platforms-plan.md.
    #[cfg(windows)]
    #[test]
    fn a_nas_only_clip_stays_on_the_nas_and_a_different_file_is_left_alone() {
        let d = temp("nas");
        let (pc, nas) = (d.join("pc"), d.join("nas"));
        put(&nas.join(r"OBS\only.mkv"), "clip");
        put(&pc.join(r"OBS\other.mkv"), "short");
        put(&nas.join(r"OBS\other.mkv"), "a different length");
        let picks = [(nas.join(r"OBS\only.mkv").to_string_lossy().into_owned(), "Apex".to_string()), (pc.join(r"OBS\other.mkv").to_string_lossy().into_owned(), "Apex".to_string())];
        let p = plan(&pc, &nas, &picks);
        let moves: Vec<_> = p.plan.moves.iter().map(|m| (m.from.clone(), m.to.clone())).collect();
        assert_eq!(moves, vec![(nas.join(r"OBS\only.mkv"), nas.join(r"Apex\only.mkv")), (pc.join(r"OBS\other.mkv"), pc.join(r"Apex\other.mkv"))]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_closest_examples_win() {
        let ex = vec![
            ("VALORANT".to_string(), v(1.0, 0.0)),
            ("VALORANT".to_string(), v(0.95, 0.1)),
            ("Counter-Strike 2".to_string(), v(0.0, 1.0)),
            ("Counter-Strike 2".to_string(), v(0.1, 0.95)),
        ];
        let g = guess(&v(0.9, 0.2), &ex, 3).unwrap();
        assert_eq!(g.game, "VALORANT");
        assert!(g.confidence > 0.6, "{g:?}");
        let g = guess(&v(0.1, 1.0), &ex, 3).unwrap();
        assert_eq!(g.game, "Counter-Strike 2");
    }

    /// Real clips: REWIND_DETECT="<onnxruntime.dll>|<model.onnx>|<clips folder>"
    /// Leave-one-out accuracy over sorted game folders, then guesses for clips
    /// in REWIND_DETECT_UNSORTED (a folder).
    /// cargo test live_game_detect -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_game_detect() {
        crate::ffmpeg::init(None);
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let Ok(spec) = std::env::var("REWIND_DETECT") else { return };
        let parts: Vec<&str> = spec.split('|').collect();
        let mut model = Model::load(Path::new(parts[0]), Path::new(parts[1])).expect("model");
        let root = Path::new(parts[2]);
        let skip = ["desktop", "edits", "imports", "obs", "exports", "temp", "render", "recordings", "screen recording", "funny not cool", "funnysiegevideo", "yt edit", "youtube videos", "edit", "other"];
        let mut examples: Vec<(String, Vec<f32>, String)> = Vec::new();
        let t = std::time::Instant::now();
        for g in std::fs::read_dir(root).unwrap().flatten() {
            let name = g.file_name().to_string_lossy().into_owned();
            if !g.path().is_dir() || skip.contains(&name.to_lowercase().as_str()) || name.starts_with('.') {
                continue;
            }
            let mut files: Vec<_> = std::fs::read_dir(g.path()).unwrap().flatten().map(|e| e.path()).filter(|p| crate::library::is_video(p)).collect();
            files.sort();
            for p in files.into_iter().rev().take(10) {
                let ps = p.to_string_lossy().into_owned();
                let Ok(info) = crate::library::probe(&ps) else { continue };
                if let Some(e) = clip_print(&mut model, &ps, info.duration) {
                    examples.push((name.clone(), e, ps));
                }
            }
        }
        let n = examples.len();
        println!("{n} sorted clips fingerprinted in {:?} ({:.2} s each)", t.elapsed(), t.elapsed().as_secs_f64() / n.max(1) as f64);
        let mut right = 0;
        let mut sure_right = 0;
        let mut sure = 0;
        for i in 0..n {
            let rest: Vec<(String, Vec<f32>)> = examples.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, (g, e, _))| (g.clone(), e.clone())).collect();
            let g = guess(&examples[i].1, &rest, 5).unwrap();
            let ok = g.game == examples[i].0;
            right += ok as usize;
            if g.confidence >= 0.7 {
                sure += 1;
                sure_right += ok as usize;
            } else if !ok {
                println!("  unsure miss: {} -> {} ({:.2})", examples[i].0, g.game, g.confidence);
            }
        }
        println!("leave-one-out: {right}/{n} right; when confident (>=0.7): {sure_right}/{sure}");
        if let Ok(dir) = std::env::var("REWIND_DETECT_UNSORTED") {
            let ex: Vec<(String, Vec<f32>)> = examples.iter().map(|(g, e, _)| (g.clone(), e.clone())).collect();
            for p in std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| crate::library::is_video(p)).take(12) {
                let ps = p.to_string_lossy().into_owned();
                let Ok(info) = crate::library::probe(&ps) else { continue };
                if let Some(e) = clip_print(&mut model, &ps, info.duration) {
                    let g = guess(&e, &ex, 5).unwrap();
                    println!("  {} -> {} (confidence {:.2}, likeness {:.2})", p.file_name().unwrap().to_string_lossy(), g.game, g.confidence, g.likeness);
                }
            }
        }
    }
}

#[cfg(test)]
mod live_pipeline {
    use super::*;
    /// REWIND_DETECT_PACK=<folder with onnxruntime.dll and clip-vision-b32-q.onnx>
    /// cargo test live_detect_pipeline -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_detect_pipeline() {
        crate::ffmpeg::init(None);
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let Ok(pack) = std::env::var("REWIND_DETECT_PACK") else { return };
        init(PathBuf::from(&pack));
        assert!(installed(), "pack files missing or wrong size");
        // The clips folder to test against, e.g. %USERPROFILE%\Videos\Rewind.
        let Ok(clips) = std::env::var("REWIND_DETECT_CLIPS") else { return };
        let mut all = crate::library::scan(Path::new(&clips), &[]);
        all.sort_by(|a, b| b.modified.cmp(&a.modified));
        let mut per: HashMap<String, usize> = HashMap::new();
        all.retain(|c| {
            let n = per.entry(c.folder.clone()).or_default();
            *n += 1;
            *n <= 10
        });
        let t = std::time::Instant::now();
        let looked = all.iter().filter(|c| print_of(c).is_some()).count();
        println!("{looked}/{} clips looked at in {:?}", all.len(), t.elapsed());
        flush();
        for s in suggestions(&all, &[], &[], &[]) {
            println!("  {} [{}] -> {} ({:.2}, likeness {:.3})", s.name, s.folder, s.game, s.confidence, s.likeness);
        }
    }
}

#[cfg(test)]
mod live_install {
    /// Downloads the real pack into a temp folder: cargo test live_detect_install -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_detect_install() {
        let d = std::env::temp_dir().join("rewind-ai-install-test");
        let _ = std::fs::remove_dir_all(&d);
        super::init(d.clone());
        let t = std::time::Instant::now();
        super::install(|_| {}).unwrap();
        assert!(super::installed());
        println!("installed in {:?}", t.elapsed());
        let _ = std::fs::remove_dir_all(d);
    }
}
