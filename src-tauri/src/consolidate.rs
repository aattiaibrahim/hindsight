//! "Move into Hindsight": tidy another recorder's clips (Medal, OBS, NVIDIA, any
//! linked folder) into Hindsight's own layout, `<clips>/<Game>/<Game> <date time>.<ext>`.
//!
//! - Games merge with Hindsight's existing folders across spellings ("Valorant"
//!   joins "VALORANT"); files at the top of a folder (OBS) go in a folder named
//!   after the recorder.
//! - Same drive: an instant rename. Another drive: copy to a hidden partial
//!   file, check the size, rename, and only then delete the original.
//! - Every finished move is written to an undo log straight away, so even a
//!   crash halfway can be undone. Undo puts files back where they were.
//! - Files changed in the last 15 s (still being written) are left alone.

use crate::library::{self, LinkedRoot};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const SETTLE: Duration = Duration::from_secs(15);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    #[serde(default)]
    pub size: u64,
    /// Copied (the original stays where it was) rather than moved
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub copied: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub label: String,
    pub count: usize,
    pub bytes: u64,
    /// The Hindsight folders clips will land in, with how many each
    pub folders: Vec<(String, usize)>,
    /// A few "old name → new name" examples
    pub examples: Vec<(String, String)>,
    /// Moving to another drive copies the data (slower) instead of renaming
    pub other_drive: bool,
    /// Copy into Hindsight and leave the originals where they are
    pub copy: bool,
    #[serde(skip)]
    pub moves: Vec<Move>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub moved: usize,
    /// (file, reason)
    pub failed: Vec<(String, String)>,
    /// Files left where they were because the move was stopped.
    #[serde(default)]
    pub stopped: usize,
}

/// Same idea as the library's game chips: "Counter-Strike 2" == "counter strike 2".
pub(crate) fn game_key(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

pub(crate) fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let t = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_end_matches(['.', ' ']).to_string();
    if t.is_empty() { "Other".into() } else { t }
}

fn drive(p: &Path) -> String {
    p.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn settled(p: &Path) -> bool {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() >= SETTLE)
        .unwrap_or(false)
}

/// Work out where every clip in `root` would go. Nothing is touched.
pub fn plan(root: &LinkedRoot, clips_dir: &Path, avoid: &[PathBuf]) -> Plan {
    plan_many(std::slice::from_ref(root), clips_dir, avoid)
}

/// The same for several folders at once ("Move all"), planned together so two
/// recorders can never pick the same new name. `avoid` may include the roots
/// themselves: each root only skips the others.
pub fn plan_many(roots: &[LinkedRoot], clips_dir: &Path, avoid: &[PathBuf]) -> Plan {
    let mut found = Vec::new();
    for root in roots {
        let others: Vec<PathBuf> = avoid.iter().filter(|a| !library::same_path(a, &root.path)).cloned().collect();
        library::scan_linked(root, &others, &mut found);
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| seen.insert(c.path.to_lowercase()));
    // Hindsight's existing game folders, by key.
    let mut existing: Vec<String> = fs::read_dir(clips_dir)
        .map(|r| r.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut plan = Plan {
        label: roots.iter().map(|r| r.label.as_str()).collect::<Vec<_>>().join(", "),
        other_drive: roots.iter().any(|r| drive(&r.path) != drive(clips_dir)),
        ..Default::default()
    };
    found.sort_by_key(|c| c.modified);
    for c in found {
        let from = PathBuf::from(&c.path);
        if !settled(&from) {
            continue;
        }
        // Reuse Hindsight's folder for this game, or one this plan already created
        // (so "Valorant" and "VALORANT" from two recorders share one folder).
        let folder = match existing.iter().find(|e| game_key(e) == game_key(&c.folder)) {
            Some(f) => f.clone(),
            None => {
                let f = safe_name(&c.folder);
                existing.push(f.clone());
                f
            }
        };
        let ext = from.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "mp4".into());
        let when = chrono::DateTime::from_timestamp_millis(c.modified)
            .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H-%M-%S").to_string())
            .unwrap_or_else(|| "unknown date".into());
        let dir = clips_dir.join(&folder);
        let mut to = dir.join(format!("{folder} {when}.{ext}"));
        let mut n = 2;
        while to.exists() || taken.contains(&to.to_string_lossy().to_lowercase()) {
            to = dir.join(format!("{folder} {when} ({n}).{ext}"));
            n += 1;
        }
        taken.insert(to.to_string_lossy().to_lowercase());
        if plan.examples.len() < 3 {
            plan.examples.push((
                from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                format!("{folder}\\{}", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            ));
        }
        match plan.folders.iter_mut().find(|(f, _)| *f == folder) {
            Some((_, k)) => *k += 1,
            None => plan.folders.push((folder.clone(), 1)),
        }
        plan.bytes += c.size;
        plan.moves.push(Move { from, to, size: c.size, copied: false });
    }
    plan.count = plan.moves.len();
    plan.folders.sort_by(|a, b| b.1.cmp(&a.1));
    plan
}

/// Move one file (or a whole folder): rename if possible, else copy + verify + delete the original.
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    // "Valorant" → "VALORANT": the same entry on Windows and most NAS shares, so `to`
    // "exists" already. Rename through a temporary name so the new spelling sticks.
    if from != to && from.to_string_lossy().eq_ignore_ascii_case(&to.to_string_lossy()) {
        let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = from.with_file_name(format!(".{name}.rewind-rename"));
        fs::rename(from, &tmp).map_err(|e| e.to_string())?;
        return fs::rename(&tmp, to).map_err(|e| {
            let _ = fs::rename(&tmp, from);
            e.to_string()
        });
    }
    if to.exists() {
        return Err("something with that name already exists".into());
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Different drive (or rename refused): copy safely, then remove the original.
    let size = fs::metadata(from).map_err(|e| e.to_string())?.len();
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = to.with_file_name(format!(".{name}.rewind-partial"));
    // Read back and compared byte for byte (like NAS sync) before the original is
    // removed: a crash or a bad drive mid-copy leaves the original as it was.
    let res = (|| {
        crate::nas::copy_verified(from, &tmp, &mut |_, _| {})?;
        if fs::metadata(&tmp).map(|m| m.len()).ok() != Some(size) {
            return Err("the copy came out the wrong size".to_string());
        }
        fs::rename(&tmp, to).map_err(|e| e.to_string())
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::remove_file(from) {
        // Keep the original rather than lose anything; the copy is already in place.
        let _ = fs::remove_file(to);
        return Err(format!("couldn't remove the original ({e}), so nothing was moved"));
    }
    Ok(())
}

/// Copy one file, keeping the original: a hidden partial file first, size checked, then renamed.
pub(crate) fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err("something with that name already exists".into());
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let size = fs::metadata(from).map_err(|e| e.to_string())?.len();
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = to.with_file_name(format!(".{name}.rewind-partial"));
    let res = (|| {
        fs::copy(from, &tmp).map_err(|e| e.to_string())?;
        if fs::metadata(&tmp).map(|m| m.len()).ok() != Some(size) {
            return Err("the copy came out the wrong size".to_string());
        }
        crate::library::flush_to_disk(&tmp)?;
        fs::rename(&tmp, to).map_err(|e| e.to_string())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// The same plan, but copying: the clips stay where they are, a copy lands in Hindsight.
pub fn as_copy(mut plan: Plan) -> Plan {
    plan.copy = true;
    for m in &mut plan.moves {
        m.copied = true;
    }
    plan
}

/// Before each file: carry on, wait a moment (a game is running), or stop here.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hold {
    Go,
    Wait,
    Stop,
}

/// Carry out a plan. Each finished move is logged immediately (one JSON line).
pub fn run(plan: &Plan, log: &Path, progress: impl FnMut(usize, usize)) -> Outcome {
    run_held(plan, log, progress, || Hold::Go)
}

/// `run`, asking `hold` before each file: a long move in the background waits while
/// you play or a clip saves, and can be stopped (what's moved stays moved, and logged,
/// so Undo still puts it back).
pub fn run_held(plan: &Plan, log: &Path, mut progress: impl FnMut(usize, usize), hold: impl Fn() -> Hold) -> Outcome {
    let mut out = Outcome::default();
    if let Some(d) = log.parent() {
        let _ = fs::create_dir_all(d);
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(log).ok();
    for (i, m) in plan.moves.iter().enumerate() {
        loop {
            match hold() {
                Hold::Go => break,
                Hold::Wait => std::thread::sleep(std::time::Duration::from_secs(2)),
                Hold::Stop => {
                    out.stopped = plan.moves.len() - i;
                    progress(i, plan.moves.len());
                    return out;
                }
            }
        }
        progress(i, plan.moves.len());
        let done = if m.copied { copy_file(&m.from, &m.to) } else { move_file(&m.from, &m.to) };
        match done {
            Ok(()) => {
                out.moved += 1;
                if let (Some(f), Ok(line)) = (f.as_mut(), serde_json::to_string(m)) {
                    let _ = writeln!(f, "{line}");
                    let _ = f.flush();
                }
            }
            Err(e) => out.failed.push((m.from.to_string_lossy().into_owned(), e)),
        }
    }
    progress(plan.moves.len(), plan.moves.len());
    out
}

pub fn read_log(log: &Path) -> Vec<Move> {
    fs::read_to_string(log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Put every file in the log back where it came from (newest first). The log is
/// removed once everything is back; anything that couldn't go back stays in it.
pub fn undo(log: &Path) -> Outcome {
    let mut out = Outcome::default();
    let mut left = Vec::new();
    for m in read_log(log).into_iter().rev() {
        if !m.to.exists() {
            out.failed.push((m.to.to_string_lossy().into_owned(), "it's not there anymore".into()));
            continue;
        }
        if m.copied {
            // The original never moved: undoing a copy removes the copy, if it's still the same file.
            let same = fs::metadata(&m.to).map(|x| x.len()).ok() == Some(m.size) && m.from.exists();
            match if same { fs::remove_file(&m.to).map_err(|e| e.to_string()) } else { Err("it changed since it was copied, so it was left".into()) } {
                Ok(()) => out.moved += 1,
                Err(e) => {
                    out.failed.push((m.to.to_string_lossy().into_owned(), e));
                    left.push(m);
                }
            }
            continue;
        }
        match move_file(&m.to, &m.from) {
            Ok(()) => out.moved += 1,
            Err(e) => {
                out.failed.push((m.to.to_string_lossy().into_owned(), e));
                left.push(m);
            }
        }
    }
    if left.is_empty() {
        let _ = fs::remove_file(log);
    } else {
        let text: Vec<String> = left.iter().rev().filter_map(|m| serde_json::to_string(m).ok()).collect();
        let _ = fs::write(log, text.join("\n") + "\n");
    }
    out
}

#[cfg(test)]
mod hold_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_held_move_waits_then_goes_and_a_stop_leaves_the_rest() {
        let d = std::env::temp_dir().join(format!("hs-hold-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("from")).unwrap();
        let mut plan = Plan::default();
        for i in 0..4 {
            let f = d.join("from").join(format!("{i}.mp4"));
            fs::write(&f, b"x").unwrap();
            plan.moves.push(Move { from: f, to: d.join("to").join(format!("{i}.mp4")), size: 1, copied: false });
        }
        // Wait twice (a game), then move two, then Stop.
        let asked = Cell::new(0);
        let out = run_held(&plan, &d.join("log.jsonl"), |_, _| {}, || {
            asked.set(asked.get() + 1);
            match asked.get() {
                1 | 2 => Hold::Wait,
                3 | 4 => Hold::Go,
                _ => Hold::Stop,
            }
        });
        assert_eq!((out.moved, out.stopped), (2, 2));
        assert!(d.join("to").join("1.mp4").is_file() && d.join("from").join("2.mp4").is_file());
        assert_eq!(read_log(&d.join("log.jsonl")).len(), 2, "what moved is logged, so Undo works");
        let _ = fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn old_file(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![1u8; bytes]).unwrap();
        // Backdate it so it counts as finished.
        let t = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn moves_into_matching_game_folders_and_undo_puts_everything_back() {
        let d = std::env::temp_dir().join(format!("rewind-consolidate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (clips, medal) = (d.join("Hindsight"), d.join("Medal"));
        fs::create_dir_all(clips.join("VALORANT")).unwrap();
        old_file(&medal.join("Clips/Valorant/clip one.mp4"), 100);
        old_file(&medal.join("Clips/Valorant/clip two.mp4"), 200);
        old_file(&medal.join("Clips/Tom Clancys Rainbow Six Siege/r6.mp4"), 300);
        old_file(&medal.join("Thumbnails/t.mp4"), 5); // not a clip folder
        fs::create_dir_all(medal.join("Clips/Valorant")).unwrap();
        let fresh = medal.join("Clips/Valorant/still writing.mp4");
        fs::write(&fresh, b"x").unwrap(); // modified just now: left alone

        let root = LinkedRoot { path: medal.clone(), label: "Medal".into() };
        let plan = plan(&root, &clips, &[clips.clone()]);
        assert_eq!(plan.count, 3, "{:?}", plan.moves);
        assert_eq!(plan.bytes, 600);
        assert!(!plan.other_drive);
        // "Valorant" joins Hindsight's "VALORANT".
        let names: Vec<_> = plan.folders.iter().map(|(f, n)| format!("{f}={n}")).collect();
        assert_eq!(names, vec!["VALORANT=2", "Tom Clancys Rainbow Six Siege=1"]);
        // Two clips with the same timestamp get distinct names.
        let tos: std::collections::HashSet<_> = plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(tos.len(), 3);
        assert!(plan.moves.iter().all(|m| m.to.file_name().unwrap().to_string_lossy().starts_with(m.to.parent().unwrap().file_name().unwrap().to_str().unwrap())));

        let log = d.join("moves").join("1.jsonl");
        let out = run(&plan, &log, |_, _| {});
        assert_eq!((out.moved, out.failed.len()), (3, 0));
        assert!(plan.moves.iter().all(|m| m.to.exists() && !m.from.exists()));
        assert!(fresh.exists(), "a file still being written must not move");
        assert_eq!(read_log(&log).len(), 3);

        let back = undo(&log);
        assert_eq!((back.moved, back.failed.len()), (3, 0));
        assert!(plan.moves.iter().all(|m| m.from.exists() && !m.to.exists()));
        assert_eq!(fs::metadata(medal.join("Clips/Tom Clancys Rainbow Six Siege/r6.mp4")).unwrap().len(), 300);
        assert!(!log.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn top_level_files_go_in_a_folder_named_after_the_recorder() {
        let d = std::env::temp_dir().join(format!("rewind-consolidate-obs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (clips, obs) = (d.join("Hindsight"), d.join("OBS"));
        fs::create_dir_all(&clips).unwrap();
        old_file(&obs.join("2026-11-03 10-00-00.mkv"), 10);
        let plan = plan(&LinkedRoot { path: obs, label: "OBS".into() }, &clips, &[clips.clone()]);
        assert_eq!(plan.folders, vec![("OBS".to_string(), 1)]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn moving_several_folders_together_never_picks_the_same_name() {
        let d = std::env::temp_dir().join(format!("rewind-consolidate-all-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (clips, medal, nvidia) = (d.join("Hindsight"), d.join("Medal"), d.join("NVIDIA"));
        fs::create_dir_all(&clips).unwrap();
        // Same game, same second, from two recorders.
        old_file(&medal.join("Clips/Valorant/a.mp4"), 10);
        old_file(&nvidia.join("VALORANT/b.mp4"), 20);
        let roots = vec![LinkedRoot { path: medal.clone(), label: "Medal".into() }, LinkedRoot { path: nvidia.clone(), label: "NVIDIA".into() }];
        let avoid = vec![clips.clone(), medal.clone(), nvidia.clone()];
        let plan = plan_many(&roots, &clips, &avoid);
        assert_eq!(plan.count, 2);
        assert_eq!(plan.label, "Medal, NVIDIA");
        assert_eq!(plan.folders.len(), 1, "one game folder for both spellings: {:?}", plan.folders);
        assert_ne!(plan.moves[0].to, plan.moves[1].to);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn folder_names_are_safe_on_windows() {
        assert_eq!(safe_name("Rainbow Six: Siege"), "Rainbow Six Siege");
        assert_eq!(safe_name("  A/B?  "), "A B");
        assert_eq!(safe_name("..."), "Other");
    }

    #[test]
    fn copy_keeps_the_originals_and_undo_removes_only_the_copies() {
        let base = std::env::temp_dir().join(format!("rewind-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let hdd = base.join("HDD/recording");
        let clips = base.join("Hindsight");
        old_file(&hdd.join("2024-01-01 10-00-00.mkv"), 40);
        old_file(&hdd.join("2024-01-02 10-00-00.mkv"), 50);
        fs::create_dir_all(&clips).unwrap();
        let root = LinkedRoot { path: hdd.clone(), label: "recording".into() };
        let plan = as_copy(plan(&root, &clips, &[]));
        assert!(plan.copy && plan.count == 2 && plan.bytes == 90);
        let log = base.join("copy.jsonl");
        let out = run(&plan, &log, |_, _| {});
        assert_eq!(out.moved, 2, "{:?}", out.failed);
        assert_eq!(fs::read_dir(&hdd).unwrap().count(), 2, "originals stay");
        let copies: Vec<_> = plan.moves.iter().map(|m| m.to.clone()).collect();
        assert!(copies.iter().all(|c| c.exists()));
        let back = undo(&log);
        assert_eq!(back.moved, 2, "{:?}", back.failed);
        assert!(copies.iter().all(|c| !c.exists()), "copies removed");
        assert_eq!(fs::read_dir(&hdd).unwrap().count(), 2, "originals untouched");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn never_overwrites() {
        let d = std::env::temp_dir().join(format!("rewind-consolidate-ow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (a, b) = (d.join("a.mp4"), d.join("b.mp4"));
        old_file(&a, 1);
        old_file(&b, 2);
        assert!(move_file(&a, &b).is_err());
        assert_eq!(fs::metadata(&b).unwrap().len(), 2);
        assert!(a.exists());
        let _ = fs::remove_dir_all(&d);
    }
}
