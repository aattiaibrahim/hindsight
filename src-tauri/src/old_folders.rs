//! Old recorder folders with no clips left in them.
//!
//! Once every clip from a recorder's folder (Medal, OBS, Xbox Game Bar, …) has
//! moved into Hindsight, or the folder is simply empty, it's just left behind.
//! Hindsight offers to remove it, but only when all of this holds:
//! - not one video is left anywhere inside it;
//! - a move log shows clips really were moved out of it, or what's left is
//!   small (a few settings or thumbnail files);
//! - it isn't a drive, a Windows folder (Videos, Desktop…), or a folder that
//!   holds Hindsight's own clips or the NAS.
//!
//! The folders looked at: linked folders, the recorders' own folders, and the
//! folders the move logs moved clips out of (a moved-from folder is unlinked,
//! so it would otherwise never be offered).
//!
//! Removing it sends it to the Recycle Bin and stops showing it in the library.
//! Undo for the move still works afterwards: moving a clip back recreates its folder.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OldFolder {
    pub path: String,
    /// The folder's own name ("Medal")
    pub label: String,
    /// Clips that were moved out of it
    pub moved: usize,
    /// Other files still in it (thumbnails, settings…) and their size
    pub files: usize,
    pub bytes: u64,
}

/// `a` is `b` or a folder inside it.
fn within(a: &Path, b: &Path) -> bool {
    let (a, b) = (a.to_string_lossy().to_lowercase().replace('/', "\\"), b.to_string_lossy().to_lowercase().replace('/', "\\"));
    let b = b.trim_end_matches('\\');
    a == b || a.starts_with(&format!("{b}\\"))
}

/// Folders that must never be offered: drives, the user's own Windows folders,
/// and anything holding Hindsight's clips or the NAS.
fn protected(p: &Path, keep: &[PathBuf]) -> bool {
    if p.parent().is_none() || p.components().count() <= 2 {
        return true; // a whole drive
    }
    if keep.iter().any(|k| within(k, p)) {
        return true;
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home = PathBuf::from(home);
        if within(&home, p) {
            return true; // the user folder itself, or one above it
        }
        for f in ["Videos", "Desktop", "Documents", "Downloads", "Pictures", "Music", "OneDrive", "AppData"] {
            if library_same(&home.join(f), p) {
                return true;
            }
        }
    }
    false
}

fn library_same(a: &Path, b: &Path) -> bool {
    within(a, b) && within(b, a)
}

/// Videos, other files and their bytes inside `dir` (all the way down).
fn contents(dir: &Path) -> (usize, usize, u64) {
    let (mut videos, mut files, mut bytes) = (0, 0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            videos += 1; // can't look inside: treat as not empty, to be safe
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if crate::library::is_video(&p) || p.to_string_lossy().ends_with(".rewind-partial") {
                videos += 1;
            } else {
                files += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (videos, files, bytes)
}

/// How many clips the move logs say were moved (not copied) out of `dir`.
fn moved_out_of(dir: &Path, logs: &Path) -> usize {
    let Ok(rd) = fs::read_dir(logs) else { return 0 };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .flat_map(|log| crate::consolidate::read_log(&log))
        .filter(|m| !m.copied && within(&m.from, dir))
        .count()
}

/// Left in a folder nothing was moved out of, it still counts as empty (a
/// recorder's settings, thumbnails or desktop.ini).
const SMALL_FILES: usize = 20;
const SMALL_BYTES: u64 = 50 * 1024 * 1024;

/// The folder each move log moved clips out of: where all its moves came from.
fn log_roots(logs: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(logs) else { return Vec::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|log| {
            let dirs: Vec<PathBuf> = crate::consolidate::read_log(&log).into_iter().filter(|m| !m.copied).filter_map(|m| m.from.parent().map(Path::to_path_buf)).collect();
            let mut root = dirs.first()?.clone();
            for d in &dirs[1..] {
                while !within(d, &root) {
                    root = root.parent()?.to_path_buf();
                }
            }
            Some(root)
        })
        .collect()
}

/// Recorder folders with no clips left in them. `candidates`: linked folders and
/// the recorders' own folders (the move logs' folders are added here).
/// `keep`: Hindsight's clips folder and the NAS. `logs`: the move logs folder.
pub fn find(candidates: &[String], keep: &[PathBuf], logs: &Path) -> Vec<OldFolder> {
    let mut all: Vec<PathBuf> = candidates.iter().map(PathBuf::from).chain(log_roots(logs)).collect();
    all.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    all.dedup_by(|a, b| within(a, b) && within(b, a));
    let offers: Vec<OldFolder> = all
        .into_iter()
        .filter(|p| p.is_dir() && !protected(p, keep))
        .filter_map(|p| {
            let moved = moved_out_of(&p, logs);
            let (videos, files, bytes) = contents(&p);
            let empty_enough = moved > 0 || (files <= SMALL_FILES && bytes <= SMALL_BYTES);
            (videos == 0 && empty_enough).then(|| OldFolder {
                label: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned()),
                path: p.to_string_lossy().into_owned(),
                moved,
                files,
                bytes,
            })
        })
        .collect();
    // One offer per folder: an emptied folder inside another offered one goes with it.
    offers.iter().filter(|o| !offers.iter().any(|x| x.path != o.path && within(Path::new(&o.path), Path::new(&x.path)))).cloned().collect()
}

/// Check `path` again (it has to be offered right now), then send it to the Recycle Bin.
pub fn remove(path: &str, linked: &[String], keep: &[PathBuf], logs: &Path) -> Result<(), String> {
    if !find(linked, keep, logs).iter().any(|f| f.path == path) {
        return Err("That folder isn't safe to remove any more (something is in it again). Nothing was deleted.".into());
    }
    let p = Path::new(path);
    #[cfg(test)]
    {
        fs::remove_dir_all(p).map_err(|e| e.to_string())
    }
    #[cfg(not(test))]
    {
        // Its leftovers are small; still, never delete for good if the Recycle Bin would.
        for e in walk_files(p) {
            crate::library::recycle_check(&e)?;
        }
        trash::delete(p).map_err(|e| format!("Couldn't move it to the Recycle Bin: {e}"))
    }
}

#[cfg(not(test))]
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hindsight-old-{name}-{}", std::process::id())).join("Recorders");
        let _ = fs::remove_dir_all(d.parent().unwrap());
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn log(logs: &Path, from: &Path, copied: bool) {
        fs::create_dir_all(logs).unwrap();
        let m = crate::consolidate::Move { from: from.to_path_buf(), to: PathBuf::from(r"C:\x\y.mp4"), size: 1, copied };
        fs::write(logs.join("20260101-000000-Medal.jsonl"), serde_json::to_string(&m).unwrap() + "\n").unwrap();
    }

    #[test]
    fn offers_only_emptied_folders_and_removes_them() {
        let base = tmp("offer");
        let medal = base.join("Medal");
        let logs = base.join("moves");
        fs::create_dir_all(medal.join("Clips/Valorant")).unwrap();
        fs::write(medal.join("settings.json"), b"{}").unwrap();
        fs::write(medal.join("Clips/Valorant/a.mp4"), b"v").unwrap();
        let linked = vec![medal.to_string_lossy().into_owned()];
        let keep = vec![base.join("Hindsight")];

        // No move yet: never offered.
        assert!(find(&linked, &keep, &logs).is_empty());
        // Moved, but a clip is still there: not offered.
        log(&logs, &medal.join("Clips/Valorant/b.mp4"), false);
        assert!(find(&linked, &keep, &logs).is_empty());
        // Every clip gone: offered, with what's left in it.
        fs::remove_file(medal.join("Clips/Valorant/a.mp4")).unwrap();
        let found = find(&linked, &keep, &logs);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].label.as_str(), found[0].moved, found[0].files), ("Medal", 1, 1));
        // A clip turning up again is caught at the last moment.
        fs::write(medal.join("new.mkv"), b"v").unwrap();
        assert!(remove(&found[0].path, &linked, &keep, &logs).is_err());
        assert!(medal.exists());
        fs::remove_file(medal.join("new.mkv")).unwrap();
        remove(&found[0].path, &linked, &keep, &logs).unwrap();
        assert!(!medal.exists());
        let _ = fs::remove_dir_all(base.parent().unwrap());
    }

    #[test]
    fn moved_from_folders_and_empty_recorder_folders_are_offered() {
        let base = tmp("roots");
        let logs = base.join("moves");
        // Medal: clips moved out of two subfolders, the folder itself no longer linked.
        let medal = base.join("Medal");
        fs::create_dir_all(medal.join("Clips/Valorant")).unwrap();
        fs::create_dir_all(medal.join("Edits")).unwrap();
        for i in 0..30 {
            fs::write(medal.join(format!("thumb{i}.jpg")), b"t").unwrap(); // more than SMALL_FILES
        }
        fs::create_dir_all(&logs).unwrap();
        let mv = |from: PathBuf| serde_json::to_string(&crate::consolidate::Move { from, to: PathBuf::from(r"C:\x\y.mp4"), size: 1, copied: false }).unwrap();
        fs::write(logs.join("20260101-000000-Medal.jsonl"), format!("{}\n{}\n", mv(medal.join("Clips/Valorant/a.mp4")), mv(medal.join("Edits/b.mp4")))).unwrap();
        // Xbox Game Bar: never moved from, but empty apart from desktop.ini.
        let captures = base.join("Captures");
        fs::create_dir_all(&captures).unwrap();
        fs::write(captures.join("desktop.ini"), b"[x]").unwrap();
        // A folder with lots of other stuff and nothing moved out: not offered.
        let busy = base.join("Busy");
        fs::create_dir_all(&busy).unwrap();
        for i in 0..30 {
            fs::write(busy.join(format!("doc{i}.txt")), b"d").unwrap();
        }
        let cands: Vec<String> = [&captures, &busy, &medal.join("Clips")].iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let mut found: Vec<String> = find(&cands, &[], &logs).into_iter().map(|f| f.label).collect();
        found.sort();
        assert_eq!(found, vec!["Captures".to_string(), "Medal".to_string()], "Medal once (its Clips folder is inside it)");
        let _ = fs::remove_dir_all(base.parent().unwrap());
    }

    #[test]
    fn copies_and_protected_folders_are_never_offered() {
        let base = tmp("protect");
        let obs = base.join("OBS");
        let logs = base.join("moves");
        fs::create_dir_all(&obs).unwrap();
        // Copied in (the originals stay) and a clip still there: not offered.
        log(&logs, &obs.join("a.mp4"), true);
        fs::write(obs.join("a.mp4"), b"v").unwrap();
        let linked = vec![obs.to_string_lossy().into_owned()];
        assert!(find(&linked, &[], &logs).is_empty());
        fs::remove_file(obs.join("a.mp4")).unwrap();
        // Moved, but it holds Hindsight's own clips folder: not offered.
        log(&logs, &obs.join("a.mp4"), false);
        assert_eq!(find(&linked, &[], &logs).len(), 1);
        assert!(find(&linked, &[obs.join("Hindsight")], &logs).is_empty());
        // Drives and the user's own folders.
        assert!(protected(Path::new(r"C:\"), &[]));
        assert!(!protected(Path::new(r"D:\Medal"), &[]), "a recorder folder at the top of a drive is fine");
        if let Ok(home) = std::env::var("USERPROFILE") {
            assert!(protected(&PathBuf::from(&home).join("Videos"), &[]));
            assert!(protected(&PathBuf::from(&home), &[]));
            assert!(!protected(&PathBuf::from(&home).join("Videos").join("Medal"), &[]));
        }
        let _ = fs::remove_dir_all(base.parent().unwrap());
    }
}

#[cfg(test)]
mod live {
    /// What would be offered on this PC (read-only): cargo test --lib live_old_folders -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_old_folders() {
        let home = std::env::var("USERPROFILE").unwrap();
        let logs = std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.rewind.app").join("moves");
        let mut cands: Vec<String> = crate::bench::recorders::clip_folders().into_iter().filter(|f| f.exists).map(|f| f.path).collect();
        cands.extend([format!(r"{home}\Videos\Captures"), r"D:\Recordings".into(), r"D:\Medal\Clips".into()]);
        let keep = vec![std::path::PathBuf::from(format!(r"{home}\Videos\Rewind"))];
        for f in super::find(&cands, &keep, &logs) {
            println!("offer: {} ({} moved out, {} other files, {} bytes)", f.path, f.moved, f.files, f.bytes);
        }
    }
}
