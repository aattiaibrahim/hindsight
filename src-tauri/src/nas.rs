//! NAS library.
//!
//! Clips always save to this PC first, so saving stays instant and works with
//! the NAS asleep or the network down. A background worker then copies each
//! finished clip to the NAS with the same `<Game>/<file>` layout, so every PC
//! shares one folder per game (file names carry the PC's name, see `pc_suffix`).
//!
//! - A copy goes to a hidden `.name.rewind-partial` file first and is renamed
//!   only once it's verified: the copy is read back from the NAS (past
//!   Windows' cache) and its SHA-256 must match the original's, byte for
//!   byte. So the NAS never shows a half-copied or garbled clip.
//! - Copied clips are remembered (nas-synced.txt), so a clip deleted on the NAS
//!   isn't copied back again.
//! - Optionally the local copy is freed up, only once the NAS copy is verified
//!   that way (clips copied by older versions are verified first), and it goes
//!   to the Recycle Bin rather than being deleted outright.
//! - Nothing is copied while a game is running (in front or tabbed out of)
//!   or a recording is going, so it never costs frames.
//! - Edits reach the NAS too: a trimmed, healed or re-exported clip that's
//!   already there replaces its NAS copy (verified the same way). The old NAS
//!   version isn't deleted: it moves to a hidden `.hindsight-replaced/<date>/`
//!   folder for 30 days, since a network folder has no Recycle Bin.
//! - The worker never touches the network on the save path: duplicate-name
//!   checks use the cached NAS listing.

use serde::Serialize;
use std::collections::HashSet;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// Files changed more recently than this may still be written (a save, a recording).
const SETTLE: Duration = Duration::from_secs(15);
const PARTIAL: &str = ".rewind-partial";

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NasState {
    pub enabled: bool,
    pub reachable: bool,
    /// Clips on this PC not on the NAS yet
    pub pending: usize,
    /// File being copied right now
    pub copying: Option<String>,
    /// Clips copied since Hindsight started
    pub copied: usize,
    pub last_error: Option<String>,
    /// When the last full pass finished (ms since 1970)
    pub last_sync: Option<i64>,
    /// Waiting because a game is running or a clip is recording
    pub paused: bool,
    /// This pass: clips and bytes to copy, and how many are done
    pub total_files: usize,
    pub done_files: usize,
    pub total_bytes: u64,
    pub done_bytes: u64,
    /// Bytes still to copy (stays set while paused)
    pub pending_bytes: u64,
    /// Reading the NAS copy back to check it
    pub checking: bool,
    /// Average speed this pass (bytes per second), counting the checks
    pub rate: f64,
    /// Time left at that speed
    pub eta_secs: Option<u64>,
    /// Freeing up space on this PC: clips done so far of this many.
    pub free_done: usize,
    pub free_total: usize,
    /// Checking a clip before freeing up its local copy (vs copying).
    pub freeing: bool,
    /// Stopped by the user (Stop in the status bar) until they press Resume.
    pub stopped: bool,
}

/// Where a sync pass is up to, for the progress display.
#[derive(Clone, Copy, Debug, Default)]
pub struct Step<'a> {
    /// The clip being copied or checked (None = the pass is over)
    pub file: Option<&'a str>,
    pub checking: bool,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: usize,
    pub total_files: usize,
    /// Freeing up space on this PC: clips checked so far, and how many to check.
    pub free_done: usize,
    pub free_total: usize,
    /// This step is checking a clip before freeing up its local copy.
    pub freeing: bool,
}

#[derive(Clone, Debug)]
pub struct NasFile {
    pub path: PathBuf,
    /// `<Game>/<file>` with forward slashes
    pub rel: String,
    pub folder: String,
}

#[derive(Default)]
struct Listing {
    root: String,
    files: Vec<NasFile>,
    rels: HashSet<String>,
}

/// Marker file in a confirmed NAS folder: one id per library.
pub const MARKER: &str = ".hindsight-library";

/// What's in a folder someone picked as the NAS folder, and what syncing would
/// do there, shown before anything is copied.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    pub dir: String,
    pub reachable: bool,
    pub writable: bool,
    /// Clips already in it, and in how many game folders.
    pub clips: usize,
    pub clip_bytes: u64,
    pub games: usize,
    /// Files in it that aren't clips (documents, photos, programs…).
    pub other_files: usize,
    pub other_examples: Vec<String>,
    /// What this PC would copy into it.
    pub to_copy: usize,
    pub to_copy_bytes: u64,
    /// It was confirmed before as a Hindsight library (from this or another PC).
    pub known_library: bool,
    /// A guess at the NAS app's address, from the share's host name.
    pub server_guess: String,
    pub on_this_pc: bool,
    /// Plain warnings; empty = it looks like a clips folder.
    pub warnings: Vec<String>,
}

fn new_id() -> String {
    let mut h = Sha256::new();
    h.update(format!("{:?}{}{:?}", SystemTime::now(), std::process::id(), std::thread::current().id()));
    h.update(std::env::var("COMPUTERNAME").unwrap_or_default());
    let x = 0u8;
    h.update(format!("{:p}", &x));
    h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

pub fn read_marker(nas: &Path) -> Option<String> {
    let id = fs::read_to_string(nas.join(MARKER)).ok()?;
    let id = id.trim().to_string();
    (id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())).then_some(id)
}

/// Look through a folder before syncing into it.
pub fn check_folder(clips_dir: &Path, dir: &Path) -> FolderCheck {
    let mut c = FolderCheck { dir: dir.to_string_lossy().into_owned(), ..Default::default() };
    let d = c.dir.trim_end_matches(['\\', '/']).to_string();
    c.on_this_pc = !d.starts_with("\\\\");
    if let Some(host) = d.strip_prefix("\\\\").and_then(|r| r.split('\\').next()).filter(|h| !h.is_empty()) {
        c.server_guess = format!("http://{host}:8420");
    }
    if !dir.is_dir() {
        c.warnings.push("Can't open this folder right now. Is the NAS on and the share reachable?".into());
        return c;
    }
    c.reachable = true;
    let probe = dir.join(format!(".hindsight-write-test-{}", std::process::id()));
    c.writable = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    c.known_library = read_marker(dir).is_some();

    // What's there: clips in game folders, and anything else (two levels down is enough to tell).
    let mut games = HashSet::new();
    let mut seen = 0usize;
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((p, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 200_000 {
                break;
            }
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with('$') || name.eq_ignore_ascii_case("#recycle") {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 3 {
                    stack.push((path, depth + 1));
                }
            } else if crate::library::is_video(&path) {
                c.clips += 1;
                c.clip_bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                if depth >= 1 {
                    games.insert(path.parent().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default());
                }
            } else if !is_sidecar(&name) {
                c.other_files += 1;
                if c.other_examples.len() < 4 {
                    c.other_examples.push(name);
                }
            }
        }
    }
    c.games = games.len();

    // What this PC would copy there.
    for (src, rel, _) in clips_in(clips_dir) {
        if size(&dir.join(rel.replace('/', "\\"))).is_none() {
            c.to_copy += 1;
            c.to_copy_bytes += size(&src).unwrap_or(0);
        }
    }

    if !c.writable {
        c.warnings.push("Hindsight can't write to this folder, so nothing could be copied.".into());
    }
    if c.on_this_pc {
        c.warnings.push("This folder is on this PC, not a network share. A NAS folder usually starts with \\\\ (like \\\\nas\\clips).".into());
    }
    let parts = d.trim_start_matches("\\\\").split('\\').filter(|p| !p.is_empty()).count();
    if parts <= 1 {
        c.warnings.push("This is the top of a drive or server. Pick (or make) a folder just for clips.".into());
    }
    if c.other_files > 20 && c.other_files > c.clips {
        c.warnings.push(format!(
            "It has {} files that aren't clips (like {}). It doesn't look like a clips folder.",
            c.other_files,
            c.other_examples.join(", ")
        ));
    }
    c
}

/// Thumbnails, notes and other files recorders keep next to clips.
fn is_sidecar(name: &str) -> bool {
    let l = name.to_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".json", ".txt", ".ini", ".db", ".m4a", ".wav", ".ogg", ".mp3", ".aac", ".opus"]
        .iter()
        .any(|e| l.ends_with(e))
}

/// Mark the folder as this library (keeping an id another PC already gave it).
pub fn confirm_folder(dir: &Path) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!("Can't reach {}", dir.display()));
    }
    if let Some(id) = read_marker(dir) {
        return Ok(id);
    }
    let id = new_id();
    fs::write(dir.join(MARKER), format!("{id}\n")).map_err(|e| format!("Can't write to the NAS folder: {e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        let w: Vec<u16> = dir.join(MARKER).as_os_str().encode_wide().chain([0]).collect();
        unsafe {
            let _ = SetFileAttributesW(windows::core::PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_HIDDEN);
        }
    }
    Ok(id)
}

/// Pair with the NAS app: a one-time token file in the folder proves the folder
/// is that server's library, and the code proves someone at the NAS app agreed.
pub fn pair(dir: &Path, server: &str, code: &str) -> Result<String, String> {
    let server = server.trim().trim_end_matches('/');
    if !(server.starts_with("http://") || server.starts_with("https://")) {
        return Err("The NAS app's address starts with http://, like http://nas:8420".into());
    }
    let token = new_id();
    let file = dir.join(format!(".hindsight-pair-{token}"));
    fs::write(&file, b"").map_err(|e| format!("Can't write to the NAS folder: {e}"))?;
    let pc = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    let _ = rustls::crypto::ring::default_provider().install_default();
    let res = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
        .and_then(|c| {
            c.post(format!("{server}/api/pair"))
                .json(&serde_json::json!({ "code": code.trim(), "token": token, "pc": pc }))
                .send()
                .map_err(|e| format!("Can't reach the NAS app at {server}. Check the address and that it's running. ({e})"))
        });
    let _ = fs::remove_file(&file);
    let res = res?;
    let ok = res.status().is_success();
    let body: serde_json::Value = res.json().unwrap_or_default();
    if ok {
        Ok(format!("Paired with the NAS app ({} clips there).", body["clips"].as_u64().unwrap_or(0)))
    } else {
        Err(body["error"].as_str().unwrap_or("The NAS app said no.").to_string())
    }
}

static STATE: Mutex<Option<NasState>> = Mutex::new(None);
static LISTING: Mutex<Option<Listing>> = Mutex::new(None);
/// Set while game folders are being merged: the worker skips its passes.
static HOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Stop in the status bar: no copying or freeing up until Resume (or the app restarts).
static STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_stopped(on: bool) {
    STOPPED.store(on, std::sync::atomic::Ordering::SeqCst);
    wake();
}

fn stopped() -> bool {
    STOPPED.load(std::sync::atomic::Ordering::SeqCst)
}
/// Folder renames from a merge ("old/" → "new/", lowercase) still to apply to
/// the copied-clips record, so a clip deleted on the NAS isn't copied back under
/// its new folder name.
static RENAMED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Pause copying (true) while folders are merged, then carry on (false).
pub fn hold(on: bool) {
    HOLD.store(on, std::sync::atomic::Ordering::SeqCst);
    if !on {
        wake();
    }
}

/// A game folder was renamed by a merge: copied clips under `from` now live under `to`.
pub fn folder_renamed(from: &str, to: &str) {
    RENAMED.lock().unwrap().push((format!("{}/", from.to_lowercase()), format!("{}/", to.to_lowercase())));
}

fn apply_renames(synced: &mut HashSet<String>, renames: &[(String, String)]) {
    for (from, to) in renames {
        let moved: Vec<String> = synced.iter().filter(|k| k.starts_with(from.as_str())).cloned().collect();
        for k in moved {
            synced.remove(&k);
            synced.insert(format!("{to}{}", &k[from.len()..]));
        }
    }
}
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// Edits made in the app since the worker last looked: (old rel, new rel).
static EDITS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// Where replaced NAS versions wait, by day, before they're removed.
const REPLACED: &str = ".hindsight-replaced";
const KEEP_REPLACED_DAYS: i64 = 30;

/// A clip was replaced by an edit (trim, heal, export with "replace"). `new`
/// can differ from `old` when the edit changed the extension.
pub fn edited(s: &crate::settings::Settings, old: &Path, new: &Path) {
    if s.nas_dir.is_empty() {
        return;
    }
    let dir = Path::new(&s.clips_dir);
    if let (Some(a), Some(b)) = (rel_of(dir, old), rel_of(dir, new)) {
        EDITS.lock().unwrap().push((a, b));
        clip_saved();
    }
}

/// Move a NAS file into today's `.hindsight-replaced` folder (never overwriting).
fn set_aside(nas: &Path, rel: &str) -> Result<(), String> {
    let from = nas.join(rel.replace('/', "\\"));
    if !from.exists() {
        return Ok(());
    }
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut to = nas.join(REPLACED).join(&day).join(rel.replace('/', "\\"));
    let mut n = 2;
    while to.exists() {
        let stem = from.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        to = to.with_file_name(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::rename(&from, &to).map_err(|e| format!("couldn't set the old NAS version aside ({e})"))
}

/// Where `path` sits inside the NAS folder ("Game/clip.mp4"), ignoring case
/// (Windows shares don't care: `\\VOYAGER-1\clips` and `\\voyager-1\Clips` are one folder).
pub fn rel_on_nas(nas_dir: &str, path: &str) -> Option<String> {
    if nas_dir.is_empty() {
        return None;
    }
    let root = nas_dir.replace('/', "\\");
    let root = root.trim_end_matches('\\');
    let p = path.replace('/', "\\");
    let head = p.get(..root.len())?;
    let rest = p.get(root.len()..)?.strip_prefix('\\')?;
    (head.eq_ignore_ascii_case(root) && !rest.is_empty() && !rest.starts_with(REPLACED)).then(|| rest.replace('\\', "/"))
}

/// Replace a clip that lives on the NAS with its edit. There's no Recycle Bin on a
/// network folder, so the original isn't deleted: it moves to
/// `.hindsight-replaced/<today>/` (kept 30 days, like replaced versions from syncing).
pub fn replace_on_nas(nas_dir: &str, original: &str, edited: &str, expected_secs: Option<f64>) -> Result<String, String> {
    let nas = Path::new(nas_dir);
    let rel = rel_on_nas(nas_dir, original).ok_or("This clip isn't in your NAS folder, so it can't be replaced safely.")?;
    let edited_p = Path::new(edited);
    crate::library::verify_output(edited_p, expected_secs)?;
    let orig = Path::new(original);
    let target = orig.with_extension(crate::library::ext_of(edited_p));
    set_aside(nas, &rel).map_err(|e| format!("Saved the edit as a new file, but {e}"))?;
    // An export can change the extension; a file already using that name is set aside too.
    if target != orig && target.exists()
        && let Some(r) = rel_on_nas(nas_dir, &target.to_string_lossy())
    {
        set_aside(nas, &r)?;
    }
    fs::rename(edited_p, &target).map_err(|e| {
        format!("Couldn't move the edit into place ({e}). Your edit is saved as {} and the original is in the NAS's {REPLACED} folder.", edited_p.display())
    })?;
    Ok(target.to_string_lossy().into_owned())
}

/// Remove replaced versions older than `KEEP_REPLACED_DAYS` (by their day folder).
fn prune_replaced(nas: &Path, today: chrono::NaiveDate) {
    let Ok(days) = fs::read_dir(nas.join(REPLACED)) else { return };
    for d in days.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        if let Ok(date) = chrono::NaiveDate::parse_from_str(&name, "%Y-%m-%d")
            && (today - date).num_days() > KEEP_REPLACED_DAYS
        {
            let _ = fs::remove_dir_all(d.path());
        }
    }
}

/// Bring the NAS copy of an edited clip up to date: copy and verify the new
/// version, set the old one aside, then put the new one in its place.
fn push_edit(local: &Path, nas: &Path, old: &str, new: &str, on_copy: &mut dyn FnMut(u64, bool)) -> Result<(), String> {
    let src = local.join(new.replace('/', "\\"));
    let dest = nas.join(new.replace('/', "\\"));
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dest.with_file_name(format!(".{name}{PARTIAL}"));
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    copy_verified(&src, &tmp, on_copy).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    let set = set_aside(nas, old).and_then(|_| if old != new { set_aside(nas, new) } else { Ok(()) });
    if let Err(e) = set {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &dest).map_err(|e| e.to_string())
}

/// Clips saved over in the NAS app (written by server/src/edit.js), one JSON line each.
pub const EDIT_LOG: &str = ".hindsight-edits.jsonl";

#[derive(serde::Deserialize, Debug)]
struct NasEdit {
    rel: String,
    old: String,
    at: i64,
    size: u64,
}

/// Bring edits made in the NAS app to this PC: for each clip this PC has (and
/// synced), copy the new version down, check it byte for byte, send the old
/// copy here to the Recycle Bin and put the edit in its place. `done` is the
/// newest edit already handled for this NAS. Returns the (old, new) local paths
/// swapped, and the first error (which stops the pass, to try again later).
pub fn pull_edits(
    local: &Path,
    nas: &Path,
    done: &mut i64,
    synced: &mut HashSet<String>,
    verified: &mut HashSet<String>,
    stop: &dyn Fn() -> bool,
) -> (Vec<(PathBuf, PathBuf)>, Option<String>) {
    let mut pulled = Vec::new();
    let Ok(text) = fs::read_to_string(nas.join(EDIT_LOG)) else {
        return (pulled, None);
    };
    let mut list: Vec<NasEdit> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).filter(|e: &NasEdit| e.at > *done).collect();
    list.sort_by_key(|e| e.at);
    for e in list {
        if stop() {
            break;
        }
        let ok_rel = |r: &str| !r.is_empty() && !r.contains("..") && !r.starts_with('/') && !r.contains(':');
        let (old_key, new_key) = (rel_key(&e.old), rel_key(&e.rel));
        let old_local = local.join(e.old.replace('/', "\\"));
        let new_local = local.join(e.rel.replace('/', "\\"));
        let nas_file = nas.join(e.rel.replace('/', "\\"));
        // Not this PC's clip, never synced from here, or edited again since (a later line covers it).
        if !ok_rel(&e.old) || !ok_rel(&e.rel) || !synced.contains(&old_key) || !old_local.is_file() || size(&nas_file) != Some(e.size) {
            *done = e.at;
            continue;
        }
        let name = new_local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = new_local.with_file_name(format!(".{name}{PARTIAL}"));
        let res = copy_verified(&nas_file, &tmp, &mut |_, _| {})
            .and_then(|_| recycle(&old_local))
            .and_then(|_| if new_local != old_local && new_local.exists() { recycle(&new_local) } else { Ok(()) })
            .and_then(|_| fs::rename(&tmp, &new_local).map_err(|x| x.to_string()));
        if let Err(x) = res {
            let _ = fs::remove_file(&tmp);
            return (pulled, Some(format!("Couldn't bring the NAS edit of {name} to this PC: {x}")));
        }
        // Same date as the original (keeps its place in the library), plus 1 ms so the
        // player and thumbnails, which go by the date, load the new version.
        if let Ok(f) = fs::OpenOptions::new().write(true).open(&new_local)
            && let Ok(t) = f.metadata().and_then(|m| m.modified())
        {
            let _ = f.set_modified(t + Duration::from_millis(1));
        }
        crate::log!("[rewind] nas: pulled the NAS app's edit of {}", e.rel);
        for set in [&mut *synced, &mut *verified] {
            set.remove(&old_key);
            set.insert(new_key.clone());
        }
        *done = e.at;
        pulled.push((old_local, new_local));
    }
    (pulled, None)
}

fn pulled_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("nas-pulled.txt"))
}

/// Newest NAS edit handled, per NAS folder ("<root>|<ms>" lines).
fn load_pulled(path: &Path, root: &str) -> i64 {
    let prefix = format!("{}|", root.to_lowercase());
    fs::read_to_string(path).unwrap_or_default().lines().find_map(|l| l.strip_prefix(&prefix).and_then(|v| v.parse().ok())).unwrap_or(0)
}

fn save_pulled(path: &Path, root: &str, at: i64) {
    let prefix = format!("{}|", root.to_lowercase());
    let mut lines: Vec<String> = fs::read_to_string(path).unwrap_or_default().lines().filter(|l| !l.starts_with(&prefix)).map(str::to_string).collect();
    lines.push(format!("{prefix}{at}"));
    let _ = fs::write(path, lines.join("\n"));
}

/// " - GAMING-PC": added to clip names when a NAS is shared by several PCs.
pub fn pc_suffix() -> String {
    let name: String = std::env::var("COMPUTERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if name.is_empty() { String::new() } else { format!(" - {name}") }
}

fn rel_key(rel: &str) -> String {
    rel.replace('\\', "/").to_lowercase()
}

/// `<Game>/<file>` of a clip inside the clips folder.
fn rel_of(clips_dir: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(clips_dir).ok()?;
    Some(r.to_string_lossy().replace('\\', "/"))
}

/// Would this new clip's name clash with one already on the NAS? (cached listing, no network)
pub fn exists_on_nas(s: &crate::settings::Settings, local: &Path) -> bool {
    if s.nas_dir.is_empty() {
        return false;
    }
    let Some(rel) = rel_of(Path::new(&s.clips_dir), local) else { return false };
    let l = LISTING.lock().unwrap();
    l.as_ref().is_some_and(|l| l.root == s.nas_dir && l.rels.contains(&rel_key(&rel)))
}

/// Clips on the NAS (cached; refreshed by the worker).
pub fn listing(nas_dir: &str) -> Vec<NasFile> {
    let l = LISTING.lock().unwrap();
    match l.as_ref() {
        Some(l) if !nas_dir.is_empty() && l.root == nas_dir => l.files.clone(),
        _ => Vec::new(),
    }
}

pub fn state() -> NasState {
    STATE.lock().unwrap().clone().unwrap_or_default()
}

/// A clip was just saved: copy it once it has settled (no need to wait for the 30 s tick).
pub fn clip_saved() {
    std::thread::spawn(|| {
        std::thread::sleep(SETTLE + Duration::from_secs(1));
        wake();
    });
}

/// Run a pass now (settings changed, or "Sync now").
pub fn wake() {
    let (m, c) = &WAKE;
    *m.lock().unwrap() = true;
    c.notify_one();
}

fn is_video(p: &Path) -> bool {
    crate::library::is_video(p)
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Videos in `<root>` and its game folders (one level, like the clips folder).
fn clips_in(root: &Path) -> Vec<(PathBuf, String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return out };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            if let Ok(inner) = fs::read_dir(&p) {
                for f in inner.flatten() {
                    let fname = f.file_name().to_string_lossy().into_owned();
                    let fp = f.path();
                    if !hidden(&fname) && fp.is_file() && is_video(&fp) {
                        out.push((fp, format!("{name}/{fname}"), name.clone()));
                    }
                }
            }
        } else if is_video(&p) {
            out.push((p, name.clone(), "Other".into()));
        }
    }
    out
}

fn scan_nas(root: &Path) -> Vec<NasFile> {
    clips_in(root).into_iter().map(|(path, rel, folder)| NasFile { path, rel, folder }).collect()
}

#[derive(Default, Debug, PartialEq)]
pub struct Pass {
    pub copied: Vec<String>,
    pub removed: Vec<String>,
    pub pending: usize,
    pub pending_bytes: u64,
    /// NAS copies brought up to date after an edit
    pub updated: Vec<String>,
    pub errors: Vec<String>,
}

/// Is `a` changed more recently than `b` (by over a minute)?
fn newer(a: &Path, b: &Path) -> bool {
    let t = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    match (t(a), t(b)) {
        (Some(a), Some(b)) => a.duration_since(b).map(|d| d > Duration::from_secs(60)).unwrap_or(false),
        _ => false,
    }
}

fn settled(p: &Path, min_age: Duration) -> bool {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() >= min_age)
        .unwrap_or(false)
}

fn size(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| m.len())
}

const CHUNK: usize = 1 << 20;

fn sha256_of(mut r: impl Read, buf: &mut [u8]) -> std::io::Result<[u8; 32]> {
    let mut h = Sha256::new();
    loop {
        let n = r.read(buf)?;
        if n == 0 {
            return Ok(h.finalize().into());
        }
        h.update(&buf[..n]);
    }
}

/// SHA-256 of a file as it really is on disk (or the NAS), not as Windows
/// remembers it from writing it a moment ago: read with no cache.
fn sha256_uncached(p: &Path) -> Result<[u8; 32], String> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    // Uncached reads need a buffer aligned to the disk's sector size.
    // Uncached reads go over the network one at a time, so bigger reads are faster.
    const READ: usize = 4 << 20;
    let mut v = vec![0u8; READ + 4096];
    let off = v.as_ptr().align_offset(4096);
    let buf = &mut v[off..off + READ];
    let uncached = fs::File::options().read(true).custom_flags(FILE_FLAG_NO_BUFFERING).open(p).and_then(|f| sha256_of(f, buf));
    match uncached {
        Ok(h) => Ok(h),
        // Some shares refuse uncached reads: an ordinary read still catches a bad copy.
        Err(_) => fs::File::open(p).and_then(|f| sha256_of(f, buf)).map_err(|e| e.to_string()),
    }
}

/// Copy `src` to `tmp`, fingerprinting it on the way, then read `tmp` back
/// and check it's identical. Keeps the clip's date.
fn copy_verified(src: &Path, tmp: &Path, progress: &mut dyn FnMut(u64, bool)) -> Result<(), String> {
    let mut r = fs::File::open(src).map_err(|e| e.to_string())?;
    let modified = r.metadata().and_then(|m| m.modified()).ok();
    let mut w = fs::File::create(tmp).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        sent += n as u64;
        progress(sent, false);
    }
    if let Some(t) = modified {
        let _ = w.set_modified(t);
    }
    w.sync_all().map_err(|e| e.to_string())?;
    drop(w);
    let want: [u8; 32] = h.finalize().into();
    progress(sent, true);
    if sha256_uncached(tmp)? != want {
        return Err("the copy on the NAS didn't match the original, so it was thrown away and will be tried again".into());
    }
    Ok(())
}

/// Are the clip here and its NAS copy identical? (For clips copied before
/// copies were verified.)
fn same_contents(local: &Path, nas: &Path) -> Result<bool, String> {
    let mut buf = vec![0u8; CHUNK];
    let a = fs::File::open(local).and_then(|f| sha256_of(f, &mut buf)).map_err(|e| e.to_string())?;
    Ok(sha256_uncached(nas)? == a)
}

/// Freed-up local copies go to the Recycle Bin (tests delete outright), only
/// when they can really land there (see `library::recycle_check`).
fn recycle(p: &Path) -> Result<(), String> {
    #[cfg(test)]
    return fs::remove_file(p).map_err(|e| e.to_string());
    #[cfg(not(test))]
    {
        crate::library::recycle_check(p)?;
        trash::delete(p).map_err(|e| e.to_string())
    }
}

/// Cleanup's "Remove from this PC" for a clip that's on the NAS: only after
/// reading both and finding them identical. The NAS copy is never touched.
pub fn free_local(s: &crate::settings::Settings, path: &Path) -> Result<(), String> {
    if s.nas_dir.is_empty() {
        return Err("The NAS isn't set up.".into());
    }
    let rel = rel_of(Path::new(&s.clips_dir), path).ok_or("Only clips in Hindsight's own folders are copied to the NAS.")?;
    let dest = Path::new(&s.nas_dir).join(rel.replace('/', "\\"));
    if !dest.is_file() {
        return Err("There's no copy of it on the NAS (yet), so it was kept.".into());
    }
    if !same_contents(path, &dest)? {
        return Err("The NAS copy isn't identical to this one, so this one was kept.".into());
    }
    recycle(path)
}

/// One sync pass: copy every settled clip that isn't on the NAS yet.
/// `synced` holds the keys of clips already copied (so deleting one on the NAS sticks).
pub fn sync_once(
    local: &Path,
    nas: &Path,
    synced: &mut HashSet<String>,
    // Keys of clips whose NAS copy is known identical (only those are ever freed up here).
    verified: &mut HashSet<String>,
    // Clips replaced by an edit whose NAS copy still needs updating: (old rel, new rel).
    edits: &mut Vec<(String, String)>,
    remove_local: bool,
    min_age: Duration,
    mut on_copy: impl FnMut(Step),
    // Checked before each copy: true stops the pass (a game started).
    stop: &dyn Fn() -> bool,
) -> Result<Pass, String> {
    if !nas.is_dir() {
        return Err(format!("Can't reach {}", nas.display()));
    }
    let mut pass = Pass::default();
    let todo: Vec<_> = clips_in(local).into_iter().filter(|(p, _, _)| settled(p, min_age)).collect();
    // What this pass will copy, for the progress display.
    let (mut total_files, mut total_bytes) = (0usize, 0u64);
    for (src, rel, _) in &todo {
        if !synced.contains(&rel_key(rel)) && size(&nas.join(rel.replace('/', "\\"))).is_none() {
            total_files += 1;
            total_bytes += size(src).unwrap_or(0);
        }
    }
    let (mut done_files, mut done_bytes) = (0usize, 0u64);
    // Clips already on the NAS whose copy here will be freed up, for the progress display.
    let free_total = if remove_local {
        todo.iter()
            .filter(|(src, rel, _)| synced.contains(&rel_key(rel)) && size(src).is_some() && size(&nas.join(rel.replace('/', "\\"))) == size(src))
            .count()
    } else {
        0
    };
    let mut free_done = 0usize;

    // Edits first, so the NAS never keeps an old version next to a new one.
    let mut keep = Vec::new();
    for (old, new) in std::mem::take(edits) {
        let (old_key, new_key) = (rel_key(&old), rel_key(&new));
        // Not on the NAS yet (or deleted there): the normal copy below handles it.
        if !synced.contains(&old_key) || !local.join(new.replace('/', "\\")).exists() {
            continue;
        }
        if stop() {
            keep.push((old, new));
            continue;
        }
        on_copy(Step { file: Some(&new), checking: false, done_bytes, total_bytes, done_files, total_files, free_done, free_total, freeing: false });
        let mut progress = |n: u64, checking: bool| {
            on_copy(Step { file: Some(&new), checking, done_bytes: done_bytes + n, total_bytes, done_files, total_files, free_done, free_total, freeing: false })
        };
        match push_edit(local, nas, &old, &new, &mut progress) {
            Ok(()) => {
                synced.remove(&old_key);
                verified.remove(&old_key);
                synced.insert(new_key.clone());
                verified.insert(new_key);
                pass.updated.push(new);
            }
            Err(e) => {
                pass.errors.push(format!("{new}: couldn't update the NAS copy ({e}); will try again"));
                keep.push((old, new));
            }
        }
    }
    *edits = keep;

    for (src, rel, _) in todo {
        let key = rel_key(&rel);
        let dest = nas.join(rel.replace('/', "\\"));
        let local_size = size(&src);
        if synced.contains(&key) {
            // Edited outside the app (or by an older version): newer here, different
            // size there. Update the NAS copy rather than leave two versions.
            let nas_size = size(&dest);
            if nas_size.is_some() && nas_size != local_size && newer(&src, &dest) {
                edits.push((rel.clone(), rel.clone()));
                continue;
            }
            // Already copied once. Free up space if asked, the NAS copy is still
            // there, and it's identical (checked once, the first time).
            if !remove_local || local_size.is_none() || nas_size != local_size {
                continue;
            }
            if !verified.contains(&key) {
                if stop() {
                    continue;
                }
                on_copy(Step { file: Some(&rel), checking: true, done_bytes, total_bytes, done_files, total_files, free_done, free_total, freeing: true });
                match same_contents(&src, &dest) {
                    Ok(true) => {
                        verified.insert(key.clone());
                    }
                    Ok(false) => {
                        pass.errors.push(format!("{rel}: the NAS copy isn't identical to this one, so this one is kept"));
                        free_done += 1;
                        continue;
                    }
                    Err(e) => {
                        pass.errors.push(format!("{rel}: couldn't check the NAS copy ({e}), so this one is kept"));
                        free_done += 1;
                        continue;
                    }
                }
            }
            if stop() {
                continue;
            }
            match recycle(&src) {
                Ok(()) => pass.removed.push(rel.clone()),
                Err(e) => pass.errors.push(format!("{rel}: couldn't move the local copy to the Recycle Bin ({e})")),
            }
            free_done += 1;
            on_copy(Step { file: Some(&rel), checking: false, done_bytes, total_bytes, done_files, total_files, free_done, free_total, freeing: true });
            continue;
        }
        match size(&dest) {
            Some(n) if Some(n) == local_size => {
                synced.insert(key);
                continue;
            }
            Some(_) => {
                pass.errors.push(format!("{rel}: a different file with that name is already on the NAS"));
                continue;
            }
            None => {}
        }
        pass.pending += 1;
        if stop() {
            continue; // counted as waiting, copied on a later pass
        }
        on_copy(Step { file: Some(&rel), checking: false, done_bytes, total_bytes, done_files, total_files, free_done, free_total, freeing: false });
        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = dest.with_file_name(format!(".{name}{PARTIAL}"));
        let copied = (|| -> Result<(), String> {
            if let Some(dir) = dest.parent() {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            copy_verified(&src, &tmp, &mut |n, checking| {
                on_copy(Step { file: Some(&rel), checking, done_bytes: done_bytes + n, total_bytes, done_files, total_files, free_done, free_total, freeing: false })
            })?;
            if size(&tmp) != local_size {
                return Err("the copy came out the wrong size".into());
            }
            fs::rename(&tmp, &dest).map_err(|e| e.to_string())
        })();
        match copied {
            Ok(()) => {
                pass.pending -= 1;
                done_files += 1;
                done_bytes += local_size.unwrap_or(0);
                synced.insert(key.clone());
                verified.insert(key);
                if remove_local {
                    match recycle(&src) {
                        Ok(()) => pass.removed.push(rel.clone()),
                        Err(e) => pass.errors.push(format!("{rel}: couldn't move the local copy to the Recycle Bin ({e})")),
                    }
                }
                pass.copied.push(rel);
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                pass.errors.push(format!("{rel}: {e}"));
            }
        }
    }
    pass.pending_bytes = total_bytes.saturating_sub(done_bytes);
    on_copy(Step { file: None, checking: false, done_bytes, total_bytes, done_files, total_files, free_done, free_total, freeing: false });
    Ok(pass)
}

fn manifest_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("nas-synced.txt"))
}

/// Same format as the synced list: clips whose NAS copy was checked identical.
fn verified_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("nas-verified.txt"))
}

/// Lines are "<nas root>|<key>", so switching NAS folders starts fresh.
fn load_synced(path: &Path, root: &str) -> HashSet<String> {
    let prefix = format!("{}|", root.to_lowercase());
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(&prefix).map(str::to_string))
        .collect()
}

fn save_synced(path: &Path, root: &str, synced: &HashSet<String>) {
    let prefix = root.to_lowercase();
    let mut lines: Vec<String> = fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with(&format!("{prefix}|")))
        .map(str::to_string)
        .collect();
    lines.extend(synced.iter().map(|k| format!("{prefix}|{k}")));
    let tmp = path.with_extension("txt.tmp");
    if fs::write(&tmp, lines.join("\n")).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn publish(app: &AppHandle, f: impl FnOnce(&mut NasState)) {
    let snapshot = {
        let mut s = STATE.lock().unwrap();
        let st = s.get_or_insert_with(NasState::default);
        f(st);
        st.clone()
    };
    let _ = app.emit("nas-state", snapshot);
}

/// Background worker: a pass every 30 s, or right after `wake()`.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut synced: HashSet<String> = HashSet::new();
        let mut verified: HashSet<String> = HashSet::new();
        let mut edits: Vec<(String, String)> = Vec::new();
        let mut synced_root = String::new();
        loop {
            {
                let (m, c) = &WAKE;
                let g = m.lock().unwrap();
                let (mut g, _) = c.wait_timeout_while(g, Duration::from_secs(30), |woken| !*woken).unwrap();
                *g = false;
            }
            if HOLD.load(std::sync::atomic::Ordering::SeqCst) {
                continue; // a merge is moving folders; `hold(false)` wakes us again
            }
            let s = app.state::<crate::AppState>().settings.lock().unwrap().clone();
            if s.nas_dir.is_empty() {
                *LISTING.lock().unwrap() = None;
                publish(&app, |st| *st = NasState::default());
                continue;
            }
            // Copying reads the SSD and uses the network: wait until the game is closed.
            let app_busy = {
                let st = app.state::<crate::AppState>();
                st.engine.is_recording()
            };
            if stopped() {
                publish(&app, |st| {
                    st.enabled = true;
                    st.stopped = true;
                    st.copying = None;
                    st.freeing = false;
                });
                continue;
            }
            if crate::games::game_running() || app_busy {
                publish(&app, |st| {
                    st.enabled = true;
                    st.paused = true;
                    st.stopped = false;
                    st.copying = None;
                });
                continue;
            }
            publish(&app, |st| {
                st.paused = false;
                st.stopped = false;
            });
            // Only sync into the folder that was checked and confirmed: if the path
            // now leads somewhere else (a remapped drive, a different share), stop.
            let nas_root = PathBuf::from(&s.nas_dir);
            // Say so the moment the NAS answers (or doesn't), not when the pass ends:
            // a pass can take a long time, and one failed check (the network not up
            // yet right after a restart) used to show "Can't reach" all through it.
            let here = nas_root.is_dir();
            publish(&app, |st| {
                st.enabled = true;
                st.reachable = here;
            });
            if here {
                match read_marker(&nas_root) {
                    Some(id) if id == s.nas_library_id => {}
                    found if s.nas_library_id.is_empty() => {
                        // Set up before this check existed: adopt the folder once.
                        let id = match found {
                            Some(id) => Ok(id),
                            None => confirm_folder(&nas_root),
                        };
                        if let Ok(id) = id {
                            let st = app.state::<crate::AppState>();
                            let mut cur = st.settings.lock().unwrap();
                            if cur.nas_dir == s.nas_dir {
                                cur.nas_library_id = id;
                                let _ = crate::settings::save(&st.settings_path, &cur);
                            }
                        }
                    }
                    _ => {
                        publish(&app, |st| {
                            st.enabled = true;
                            st.reachable = true;
                            st.copying = None;
                            st.last_error = Some("Syncing stopped: this isn't the NAS folder you confirmed (its Hindsight marker is missing or different). Nothing was copied. Choose the NAS folder again in Settings to check it.".into());
                        });
                        continue;
                    }
                }
            }
            if crate::engine::saves_in_progress() > 0 {
                std::thread::sleep(Duration::from_secs(3));
                wake();
                continue;
            }
            let manifest = manifest_path(&app);
            let verified_file = verified_path(&app);
            if synced_root != s.nas_dir {
                synced = manifest.as_deref().map(|p| load_synced(p, &s.nas_dir)).unwrap_or_default();
                verified = verified_file.as_deref().map(|p| load_synced(p, &s.nas_dir)).unwrap_or_default();
                synced_root = s.nas_dir.clone();
            }
            let renames = std::mem::take(&mut *RENAMED.lock().unwrap());
            if !renames.is_empty() {
                apply_renames(&mut synced, &renames);
                if let Some(p) = &manifest {
                    save_synced(p, &s.nas_dir, &synced);
                }
            }
            let nas = PathBuf::from(&s.nas_dir);
            edits.append(&mut EDITS.lock().unwrap());
            let a = app.clone();
            let mut started: Option<std::time::Instant> = None;
            let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
            let mut last_file = String::new();
            // While freeing up space, the library is refreshed every few seconds, so a
            // freed clip moves from Local clips to the NAS-only ones right away instead
            // of when the (long) pass ends.
            let mut last_refresh = std::time::Instant::now();
            // Edits made in the NAS app come down first, so this PC never pushes an old version.
            let mut pulled_any = false;
            if let Some(pf) = pulled_path(&app) {
                let before = load_pulled(&pf, &s.nas_dir);
                let mut done = before;
                let (pulled, err) = pull_edits(Path::new(&s.clips_dir), &nas, &mut done, &mut synced, &mut verified, &|| crate::games::game_running());
                if done != before {
                    save_pulled(&pf, &s.nas_dir, done);
                }
                for (_, new) in &pulled {
                    crate::remember_edited(&app, &new.to_string_lossy());
                }
                pulled_any = !pulled.is_empty();
                if let Some(e) = err {
                    publish(&app, |st| st.last_error = Some(e));
                }
            }
            let result = sync_once(Path::new(&s.clips_dir), &nas, &mut synced, &mut verified, &mut edits, s.nas_remove_local, SETTLE, |step| {
                if step.freeing && !step.checking && last_refresh.elapsed() >= Duration::from_secs(4) {
                    last_refresh = std::time::Instant::now();
                    let _ = a.emit("clips-changed", ());
                }
                let file = step.file.map(|r| r.rsplit('/').next().unwrap_or(r).to_string());
                // Throttled: a clip is hundreds of 1 MB chunks.
                let new_file = file.as_deref().unwrap_or("") != last_file;
                if !new_file && step.file.is_some() && last_emit.elapsed() < Duration::from_millis(400) {
                    return;
                }
                last_emit = std::time::Instant::now();
                last_file = file.clone().unwrap_or_default();
                let since = *started.get_or_insert_with(std::time::Instant::now);
                let secs = since.elapsed().as_secs_f64();
                let rate = if secs > 2.0 { step.done_bytes as f64 / secs } else { 0.0 };
                let left = step.total_bytes.saturating_sub(step.done_bytes);
                publish(&a, |st| {
                    st.reachable = true; // it's working with the NAS right now
                    st.copying = file;
                    st.checking = step.checking;
                    st.freeing = step.freeing;
                    st.free_done = step.free_done;
                    st.free_total = step.free_total;
                    st.total_files = step.total_files;
                    st.done_files = step.done_files;
                    st.total_bytes = step.total_bytes;
                    st.done_bytes = step.done_bytes;
                    st.pending_bytes = left;
                    st.rate = rate;
                    st.eta_secs = (rate > 0.0 && left > 0).then(|| (left as f64 / rate) as u64);
                });
            }, &|| crate::games::game_running() || stopped());
            if let Some(p) = &manifest {
                save_synced(p, &s.nas_dir, &synced);
            }
            if let Some(p) = &verified_file {
                save_synced(p, &s.nas_dir, &verified);
            }
            let reachable = result.is_ok();
            let mut changed = pulled_any;
            match &result {
                Ok(pass) => {
                    prune_replaced(&nas, chrono::Local::now().date_naive());
                    changed = !pass.copied.is_empty() || !pass.removed.is_empty() || !pass.updated.is_empty();
                    publish(&app, |st| {
                        st.enabled = true;
                        st.reachable = true;
                        st.pending = pass.pending;
                        st.pending_bytes = pass.pending_bytes;
                        st.copied += pass.copied.len();
                        st.last_error = pass.errors.first().cloned();
                        st.last_sync = Some(now_ms());
                    });
                }
                Err(e) => publish(&app, |st| {
                    st.enabled = true;
                    st.reachable = false;
                    st.copying = None;
                    st.last_error = Some(e.clone());
                }),
            }
            // Refresh the NAS listing (other PCs' clips show up in the library).
            if reachable {
                let files = scan_nas(&nas);
                let rels: HashSet<String> = files.iter().map(|f| rel_key(&f.rel)).collect();
                let mut l = LISTING.lock().unwrap();
                let before = l.as_ref().filter(|l| l.root == s.nas_dir).map(|l| l.rels.clone());
                changed |= before.as_ref() != Some(&rels);
                *l = Some(Listing { root: s.nas_dir.clone(), files, rels });
            }
            if changed {
                let _ = app.emit("clips-changed", ());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nas_paths_are_found_whatever_the_case() {
        assert_eq!(rel_on_nas("\\\\VOYAGER\\clips", "\\\\voyager\\Clips\\Desktop\\a.mp4").as_deref(), Some("Desktop/a.mp4"));
        assert_eq!(rel_on_nas("\\\\nas\\clips\\", "//nas/clips/a.mp4").as_deref(), Some("a.mp4"));
        assert_eq!(rel_on_nas("\\\\nas\\clips", "\\\\nas\\clipsx\\a.mp4"), None);
        assert_eq!(rel_on_nas("\\\\nas\\clips", "\\\\nas\\clips\\.hindsight-replaced\\a.mp4"), None);
        assert_eq!(rel_on_nas("", "\\\\nas\\clips\\a.mp4"), None);
    }

    #[test]
    fn replacing_a_nas_clip_sets_the_original_aside_and_keeps_its_date() {
        crate::ffmpeg::init(None);
        let nas = std::env::temp_dir().join(format!("rewind-nas-replace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&nas);
        fs::create_dir_all(nas.join("Desktop")).unwrap();
        let make = |p: &Path, secs: &str| {
            let mut a = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30", "-t", secs, "-c:v", "mpeg4"]);
            a.push(p.into());
            crate::ffmpeg::run("ffmpeg", &a).unwrap();
        };
        let orig = nas.join("Desktop").join("clip.mp4");
        let edit = nas.join("Desktop").join("clip (trim).mp4");
        make(&orig, "4");
        make(&edit, "2");
        let old = fs::read(&orig).unwrap();
        let taken = crate::library::dates_of(&orig).unwrap();
        let nas_dir = nas.to_string_lossy().into_owned();
        let out = replace_on_nas(&nas_dir, &orig.to_string_lossy(), &edit.to_string_lossy(), Some(2.0)).unwrap();
        crate::library::set_dates(Path::new(&out), taken);
        assert_eq!(Path::new(&out), orig);
        assert!(!edit.exists(), "the edit moved into the clip's place");
        assert_ne!(fs::read(&orig).unwrap(), old, "the clip is the edit now");
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let kept = nas.join(REPLACED).join(day).join("Desktop").join("clip.mp4");
        assert_eq!(fs::read(&kept).unwrap(), old, "the original is set aside, not deleted");
        let now = crate::library::dates_of(&orig).unwrap();
        let ms = |t: std::time::SystemTime| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!((ms(now.0) - ms(taken.0)).abs() < 1000, "keeps the date it was taken");
        // A path outside the NAS folder is refused, not replaced.
        assert!(replace_on_nas(&nas_dir, "C:\\elsewhere\\a.mp4", &edit.to_string_lossy(), None).is_err());
        let _ = fs::remove_dir_all(&nas);
    }

    #[test]
    fn merged_folders_keep_their_copied_record() {
        let mut synced: HashSet<String> =
            ["valorant/a.mkv", "valorant x/b.mkv", "desktop/c.mkv"].iter().map(|s| s.to_string()).collect();
        apply_renames(&mut synced, &[("valorant/".into(), "valorant!/".into())]);
        let mut v: Vec<&String> = synced.iter().collect();
        v.sort();
        assert_eq!(v, vec!["desktop/c.mkv", "valorant x/b.mkv", "valorant!/a.mkv"]);
    }

    #[test]
    fn folder_check_warns_about_a_folder_that_isnt_for_clips() {
        let local = tmp("chk-local");
        let docs = tmp("chk-docs");
        fs::create_dir_all(local.join("VALORANT")).unwrap();
        fs::write(local.join("VALORANT").join("a.mkv"), vec![0u8; 1000]).unwrap();
        for i in 0..30 {
            fs::write(docs.join(format!("report {i}.docx")), b"x").unwrap();
        }
        let c = check_folder(&local, &docs);
        assert!(c.reachable && c.writable);
        assert_eq!((c.clips, c.other_files, c.to_copy, c.to_copy_bytes), (0, 30, 1, 1000));
        assert!(c.warnings.iter().any(|w| w.contains("doesn't look like a clips folder")), "{:?}", c.warnings);
        assert!(!c.known_library);

        // A clips folder: no "not a clips folder" warning, and the marker sticks.
        let nas = tmp("chk-nas");
        fs::create_dir_all(nas.join("VALORANT")).unwrap();
        fs::write(nas.join("VALORANT").join("a.mkv"), vec![0u8; 1000]).unwrap();
        let c = check_folder(&local, &nas);
        assert_eq!((c.clips, c.games, c.to_copy), (1, 1, 0));
        assert!(!c.warnings.iter().any(|w| w.contains("clips folder")), "{:?}", c.warnings);
        let id = confirm_folder(&nas).unwrap();
        assert_eq!(read_marker(&nas).as_deref(), Some(id.as_str()));
        assert_eq!(confirm_folder(&nas).unwrap(), id, "keeps the id it has");
        assert!(check_folder(&local, &nas).known_library);
        for d in [local, docs, nas] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn edits_made_in_the_nas_app_come_down_to_this_pc() {
        let local = tmp("pull-local");
        let nas = tmp("pull-nas");
        let old = clip(&local, "VALORANT/ace - PC.mkv", 1000);
        clip(&nas, "VALORANT/ace - PC.mkv", 400); // the NAS app saved a 400-byte trim over it
        clip(&local, "VALORANT/other - PC.mkv", 10);
        let log = [
            r#"{"rel":"VALORANT/ace - PC.mkv","old":"VALORANT/ace - PC.mkv","at":100,"size":400}"#,
            r#"{"rel":"VALORANT/other - PC.mkv","old":"VALORANT/other - PC.mkv","at":200,"size":999}"#,
            r#"{"rel":"VALORANT/never - OTHERPC.mkv","old":"VALORANT/never - OTHERPC.mkv","at":300,"size":5}"#,
        ];
        fs::write(nas.join(EDIT_LOG), log.join("\n")).unwrap();
        let mut synced: HashSet<String> = ["valorant/ace - pc.mkv", "valorant/other - pc.mkv"].iter().map(|s| s.to_string()).collect();
        let mut verified = synced.clone();
        let mut done = 0;
        let (pulled, err) = pull_edits(&local, &nas, &mut done, &mut synced, &mut verified, &|| false);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(pulled.len(), 1);
        assert_eq!(fs::metadata(&old).unwrap().len(), 400, "the edit replaced the clip here");
        assert_eq!(fs::metadata(local.join("VALORANT").join("other - PC.mkv")).unwrap().len(), 10, "size mismatch: left alone");
        assert_eq!(done, 300);
        // Nothing new: a second pass does nothing.
        let (again, _) = pull_edits(&local, &nas, &mut done, &mut synced, &mut verified, &|| false);
        assert!(again.is_empty());
        // A game running: nothing happens and nothing is marked done.
        let mut d0 = 0;
        let (none, _) = pull_edits(&local, &nas, &mut d0, &mut synced, &mut verified, &|| true);
        assert!(none.is_empty() && d0 == 0);
        let _ = fs::remove_dir_all(local);
        let _ = fs::remove_dir_all(nas);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rewind-nas-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn clip(root: &Path, rel: &str, bytes: usize) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![7u8; bytes]).unwrap();
        p
    }

    #[test]
    fn remove_from_this_pc_only_with_an_identical_nas_copy() {
        let (local, nas) = (tmp("f-local"), tmp("f-nas"));
        let a = clip(&local, "Game/a.mkv", 3000);
        let b = clip(&local, "Game/b.mkv", 3000);
        let c = clip(&local, "Game/c.mkv", 3000);
        fs::create_dir_all(nas.join("Game")).unwrap();
        fs::copy(&a, nas.join("Game/a.mkv")).unwrap();
        fs::write(nas.join("Game/b.mkv"), vec![9u8; 3000]).unwrap(); // same size, different bytes
        let s = crate::settings::Settings { clips_dir: local.to_string_lossy().into(), nas_dir: nas.to_string_lossy().into(), ..Default::default() };
        free_local(&s, &a).unwrap();
        assert!(!a.exists() && nas.join("Game/a.mkv").exists(), "removed here, the NAS copy untouched");
        assert!(free_local(&s, &b).unwrap_err().contains("isn't identical") && b.exists());
        assert!(free_local(&s, &c).unwrap_err().contains("no copy") && c.exists());
        // This PC's Recycle Bin takes an ordinary file.
        assert!(crate::library::recycle_check(&b).is_ok());
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn edits_update_the_nas_and_keep_the_old_version_aside() {
        let (local, nas) = (tmp("e-local"), tmp("e-nas"));
        fs::create_dir_all(&nas).unwrap();
        let a = clip(&local, "Game/a.mkv", 1000);
        let b = clip(&local, "Game/b.mkv", 1000);
        let (mut synced, mut verified, mut edits) = (HashSet::new(), HashSet::new(), Vec::new());
        let mut run = |synced: &mut HashSet<String>, verified: &mut HashSet<String>, edits: &mut Vec<(String, String)>| {
            sync_once(&local, &nas, synced, verified, edits, false, Duration::ZERO, |_| {}, &|| false).unwrap()
        };
        run(&mut synced, &mut verified, &mut edits);
        let replaced = |rel: &str| {
            let day = chrono::Local::now().format("%Y-%m-%d").to_string();
            nas.join(REPLACED).join(day).join(rel)
        };

        // Trimmed in place (same name): the NAS gets the new version, the old one is set aside.
        fs::write(&a, vec![1u8; 600]).unwrap();
        edits.push(("Game/a.mkv".into(), "Game/a.mkv".into()));
        let pass = run(&mut synced, &mut verified, &mut edits);
        assert_eq!(pass.updated, vec!["Game/a.mkv".to_string()], "{pass:?}");
        assert_eq!(fs::read(nas.join("Game/a.mkv")).unwrap(), vec![1u8; 600]);
        assert_eq!(size(&replaced("Game/a.mkv")), Some(1000));
        assert!(edits.is_empty());

        // Exported to MP4 with "replace": one clip on the NAS, not an old .mkv beside a new .mp4.
        fs::remove_file(&b).unwrap();
        clip(&local, "Game/b.mp4", 700);
        edits.push(("Game/b.mkv".into(), "Game/b.mp4".into()));
        let pass = run(&mut synced, &mut verified, &mut edits);
        assert_eq!(pass.updated, vec!["Game/b.mp4".to_string()], "{pass:?}");
        assert!(!nas.join("Game/b.mkv").exists() && nas.join("Game/b.mp4").exists());
        assert_eq!(size(&replaced("Game/b.mkv")), Some(1000));
        // The next pass doesn't copy it again.
        assert!(run(&mut synced, &mut verified, &mut edits).copied.is_empty());

        // Edited elsewhere: newer here and a different size there is picked up on its own.
        let c = clip(&local, "Game/c.mkv", 1000);
        run(&mut synced, &mut verified, &mut edits);
        fs::write(&c, vec![2u8; 400]).unwrap();
        let later = SystemTime::now() + Duration::from_secs(600);
        fs::File::options().write(true).open(&c).unwrap().set_modified(later).unwrap();
        run(&mut synced, &mut verified, &mut edits); // notices it
        let pass = run(&mut synced, &mut verified, &mut edits); // updates it
        assert_eq!(pass.updated, vec!["Game/c.mkv".to_string()], "{pass:?}");
        assert_eq!(size(&nas.join("Game/c.mkv")), Some(400));

        // Old set-aside versions go after 30 days.
        let old_day = nas.join(REPLACED).join("2020-01-01");
        fs::create_dir_all(&old_day).unwrap();
        prune_replaced(&nas, chrono::Local::now().date_naive());
        assert!(!old_day.exists() && replaced("Game/a.mkv").exists());
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn progress_counts_files_and_bytes() {
        let (local, nas) = (tmp("p-local"), tmp("p-nas"));
        clip(&local, "A/a.mkv", 3 << 20);
        clip(&local, "B/b.mkv", 1 << 20);
        fs::create_dir_all(&nas).unwrap();
        let (mut synced, mut verified) = (HashSet::new(), HashSet::new());
        let mut steps: Vec<(u64, u64, usize, usize, bool)> = Vec::new();
        let on = |s: Step| steps.push((s.done_bytes, s.total_bytes, s.done_files, s.total_files, s.checking));
        sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, on, &|| false).unwrap();
        let total = 4u64 << 20;
        assert!(steps.iter().all(|s| s.1 == total && s.3 == 2), "{steps:?}");
        assert!(steps.windows(2).all(|w| w[1].0 >= w[0].0), "bytes only go up");
        assert!(steps.iter().any(|s| s.4), "the check shows up");
        assert_eq!(steps.last().unwrap().0, total);
        assert_eq!(steps.last().unwrap().2, 2);
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn nothing_is_copied_while_a_game_is_running() {
        let (local, nas) = (tmp("g-local"), tmp("g-nas"));
        fs::create_dir_all(&nas).unwrap();
        clip(&local, "VALORANT/a.mkv", 1000);
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), true, Duration::ZERO, |_| {}, &|| true).unwrap();
        assert!(pass.copied.is_empty() && pass.removed.is_empty());
        assert_eq!(pass.pending, 1);
        assert!(local.join("VALORANT/a.mkv").exists() && !nas.join("VALORANT/a.mkv").exists());
        // Game closed: the next pass copies it.
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert_eq!(pass.copied.len(), 1);
    }

    #[test]
    fn copies_clips_into_matching_game_folders() {
        let (local, nas) = (tmp("a-local"), tmp("a-nas"));
        clip(&local, "VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv", 1000);
        clip(&local, "Desktop/Desktop 2026-11-03 10-01-00 - PC1.mkv", 500);
        clip(&local, ".rewind-save-123/part.mkv", 50); // in-progress save: never copied
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert_eq!(pass.copied.len(), 2, "{pass:?}");
        assert_eq!(pass.pending, 0);
        assert_eq!(size(&nas.join("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv")), Some(1000));
        assert!(!nas.join(".rewind-save-123").exists());
        // No leftovers from the copy-then-rename.
        assert!(fs::read_dir(nas.join("VALORANT")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(PARTIAL)));
        // Local copies are kept unless asked.
        assert!(local.join("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv").exists());
        // A second pass does nothing.
        let again = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert!(again.copied.is_empty() && again.errors.is_empty());
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn clips_still_being_written_wait() {
        let (local, nas) = (tmp("b-local"), tmp("b-nas"));
        clip(&local, "Game/new.mkv", 100);
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::from_secs(60), |_| {}, &|| false).unwrap();
        assert!(pass.copied.is_empty());
        assert!(!nas.join("Game").exists());
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn deleting_on_the_nas_sticks_and_local_removal_needs_a_verified_copy() {
        let (local, nas) = (tmp("c-local"), tmp("c-nas"));
        let a = clip(&local, "Game/a.mkv", 300);
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, |_| {}, &|| false).unwrap();
        // Deleted on the NAS: not copied back.
        fs::remove_file(nas.join("Game/a.mkv")).unwrap();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), true, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert!(pass.copied.is_empty());
        // ...and the local copy is kept, because there's no NAS copy to rely on.
        assert!(a.exists());
        // A new clip with "remove local" on: copied, verified, then removed here.
        let b = clip(&local, "Game/b.mkv", 400);
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), true, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert_eq!(pass.copied, vec!["Game/b.mkv".to_string()]);
        assert_eq!(pass.removed, vec!["Game/b.mkv".to_string()]);
        assert!(!b.exists());
        assert_eq!(size(&nas.join("Game/b.mkv")), Some(400));
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn a_garbled_nas_copy_never_frees_up_the_local_clip() {
        let (local, nas) = (tmp("v-local"), tmp("v-nas"));
        let a = clip(&local, "Game/a.mkv", 5000);
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, |_| {}, &|| false).unwrap();
        // The copy is identical and keeps the clip's date.
        assert_eq!(fs::read(&a).unwrap(), fs::read(nas.join("Game/a.mkv")).unwrap());
        let date = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(date(&a), date(&nas.join("Game/a.mkv")));
        // Like a clip copied by an older version: synced, never checked. Then the
        // NAS copy goes bad without changing size (a flaky drive or network).
        verified.clear();
        let mut bytes = fs::read(nas.join("Game/a.mkv")).unwrap();
        bytes[2500] ^= 0xff;
        fs::write(nas.join("Game/a.mkv"), bytes).unwrap();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), true, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert!(pass.removed.is_empty(), "{pass:?}");
        assert!(pass.errors[0].contains("isn't identical"), "{pass:?}");
        assert!(a.exists(), "the local clip must be kept");
        // Fixed on the NAS: now it's checked, found identical, and freed up.
        fs::copy(&a, nas.join("Game/a.mkv")).unwrap();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), true, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert_eq!(pass.removed, vec!["Game/a.mkv".to_string()]);
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn a_different_file_with_the_same_name_is_never_overwritten() {
        let (local, nas) = (tmp("d-local"), tmp("d-nas"));
        clip(&local, "Game/x.mkv", 100);
        clip(&nas, "Game/x.mkv", 999);
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        let pass = sync_once(&local, &nas, &mut synced, &mut verified, &mut Vec::new(), true, Duration::ZERO, |_| {}, &|| false).unwrap();
        assert!(pass.copied.is_empty());
        assert_eq!(pass.errors.len(), 1);
        assert_eq!(size(&nas.join("Game/x.mkv")), Some(999));
        assert!(local.join("Game/x.mkv").exists());
        let _ = (fs::remove_dir_all(&local), fs::remove_dir_all(&nas));
    }

    #[test]
    fn unreachable_nas_is_an_error_not_a_crash() {
        let local = tmp("e-local");
        let mut synced = HashSet::new();
        let mut verified = HashSet::new();
        assert!(sync_once(&local, Path::new(r"Z:\no\such\nas"), &mut synced, &mut verified, &mut Vec::new(), false, Duration::ZERO, |_| {}, &|| false).is_err());
        let _ = fs::remove_dir_all(&local);
    }

    #[test]
    fn synced_list_is_per_nas_folder() {
        let d = tmp("f-manifest");
        let p = d.join("nas-synced.txt");
        let a: HashSet<String> = ["game/a.mkv".to_string()].into();
        let b: HashSet<String> = ["game/b.mkv".to_string()].into();
        save_synced(&p, r"\\nas\one", &a);
        save_synced(&p, r"\\nas\two", &b);
        assert_eq!(load_synced(&p, r"\\nas\one"), a);
        assert_eq!(load_synced(&p, r"\\NAS\two"), b);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pc_suffix_is_safe_in_file_names() {
        let s = pc_suffix();
        assert!(s.is_empty() || (s.starts_with(" - ") && !s[3..].contains(['\\', '/', ':', '*', '?'])));
    }
}

#[cfg(test)]
mod live {
    /// Verified copy to a real share, timed: REWIND_NAS_TEST=\server\share
    /// cargo test live_nas_verify -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_nas_verify() {
        let Ok(share) = std::env::var("REWIND_NAS_TEST") else { return };
        let dir = std::path::Path::new(&share).join(".rewind-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::env::temp_dir().join("rewind-verify-src.bin");
        let data: Vec<u8> = (0..200u32 << 20).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let tmp = dir.join(".x.mkv.rewind-partial");
        let t = std::time::Instant::now();
        super::copy_verified(&src, &tmp, &mut |_, _| {}).unwrap();
        let secs = t.elapsed().as_secs_f64();
        println!("200 MB copied + verified in {secs:.1} s ({:.0} MB/s)", 200.0 / secs);
        let t = std::time::Instant::now();
        assert!(super::same_contents(&src, &tmp).unwrap());
        println!("re-check of existing copy: {:.1} s", t.elapsed().as_secs_f64());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&src);
        assert!(!dir.exists());
    }
}


#[cfg(test)]
mod live_pair {
    /// Pairs with a running NAS app. REWIND_PAIR_SERVER, REWIND_PAIR_DIR (its
    /// clips folder) and REWIND_PAIR_CODE: cargo test --lib live_pair -- --ignored
    #[test]
    #[ignore]
    fn live_pair() {
        let v = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
        let dir = v("REWIND_PAIR_DIR");
        let wrong = super::pair(std::path::Path::new(&std::env::temp_dir()), &v("REWIND_PAIR_SERVER"), &v("REWIND_PAIR_CODE"));
        println!("other folder: {wrong:?}");
        assert!(wrong.unwrap_err().contains("isn't this server's clips folder"));
        let ok = super::pair(std::path::Path::new(&dir), &v("REWIND_PAIR_SERVER"), &v("REWIND_PAIR_CODE"));
        println!("right folder: {ok:?}");
        assert!(ok.is_ok());
        assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".hindsight-pair-")), "token file left behind");
    }
}

#[cfg(test)]
mod live_scan_nas {
    /// How long listing the NAS takes (read-only): cargo test --lib live_nas_scan -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_nas_scan() {
        let root = std::path::Path::new("//voyager-1/clips");
        let t = std::time::Instant::now();
        println!("read_dir: {:?}", std::fs::read_dir(root).map(|r| r.count()));
        let files = super::scan_nas(root);
        let listed = t.elapsed();
        let t = std::time::Instant::now();
        let n = files.iter().filter(|f| std::fs::metadata(&f.path).is_ok()).count();
        let statted = t.elapsed();
        println!("listed {} NAS clips in {listed:?}; one metadata call each ({n}) took {statted:?}", files.len());
    }
}
