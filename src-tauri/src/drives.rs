//! "Find clips on your drives": look through every drive (an external HDD too)
//! for folders that hold clips, such as an old Medal folder or a "Recordings"
//! folder, so they can be shown in the library or copied or moved into Hindsight.
//!
//! - A folder counts when its name looks like a recorder's (Medal, Clips,
//!   Recording(s), Captures, OBS, ShadowPlay, NVIDIA, Highlights, Replays) and it
//!   has a clip, or when it holds at least 5 videos itself. Inside a found
//!   folder we don't look for more.
//! - Only a few levels deep, skipping Windows, program and system folders, and
//!   with a cap on how many folders are opened, so it stays quick on a big disk.
//! - Read only: nothing is touched.

use crate::library::{self, LinkedRoot};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FoundFolder {
    pub path: String,
    pub label: String,
    /// "E:"
    pub drive: String,
    pub clips: usize,
    pub bytes: u64,
    /// Already shown in the library (one of the linked folders)
    pub shown: bool,
}

const MAX_DEPTH: u32 = 4;
const MAX_DIRS: usize = 40_000;
const NAMES: [&str; 11] = ["medal", "clip", "record", "capture", "obs", "shadowplay", "nvidia", "highlight", "replay", "geforce", "outplayed"];

fn skip(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with('.')
        || n.starts_with('$')
        || ["windows", "program files", "program files (x86)", "programdata", "appdata", "system volume information", "recovery", "node_modules", "steamapps", "windowsapps", "perflogs", "msocache", "intel", "amd", "nvidia corporation"]
            .contains(&n.as_str())
        || n.contains("thumbnail")
        || n == "cache"
}

fn looks_like_clips(name: &str) -> bool {
    let n = name.to_lowercase();
    NAMES.iter().any(|k| n.contains(k))
}

/// Who probably made the clips, from the path; else the folder's own name.
pub fn label_for(path: &Path) -> String {
    let p = path.to_string_lossy().to_lowercase();
    if p.contains("medal") {
        "Medal".into()
    } else if p.contains("shadowplay") || p.contains("nvidia") || p.contains("geforce") {
        "NVIDIA".into()
    } else if p.split(['\\', '/']).any(|s| s == "obs" || s.starts_with("obs ")) {
        "OBS".into()
    } else {
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone())
    }
}

/// Look in `roots` (drive roots, or any folders) for clip folders.
/// `avoid`: Hindsight's own clips folder and the NAS, never reported or entered.
/// `shown`: the library's linked folders, reported with `shown: true`.
pub fn find(roots: &[PathBuf], avoid: &[PathBuf], shown: &[PathBuf]) -> Vec<FoundFolder> {
    let mut out = Vec::new();
    let mut budget = MAX_DIRS;
    for root in roots {
        walk(root, 0, root, avoid, shown, &mut budget, &mut out);
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

fn walk(dir: &Path, depth: u32, root: &Path, avoid: &[PathBuf], shown: &[PathBuf], budget: &mut usize, out: &mut Vec<FoundFolder>) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut videos = 0usize;
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                if !skip(&name) && !avoid.iter().any(|a| library::same_path(a, &p)) {
                    subdirs.push((p, name));
                }
            }
            Ok(t) if t.is_file() && library::is_video(&p) => videos += 1,
            _ => {}
        }
    }
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if depth > 0 && (videos >= 5 || looks_like_clips(&name)) {
        let label = label_for(dir);
        let mut found = Vec::new();
        library::scan_linked(&LinkedRoot { path: dir.to_path_buf(), label: label.clone() }, avoid, &mut found);
        if !found.is_empty() {
            out.push(FoundFolder {
                path: dir.to_string_lossy().into_owned(),
                drive: root.to_string_lossy().trim_end_matches(['\\', '/']).to_string(),
                label,
                clips: found.len(),
                bytes: found.iter().map(|c| c.size).sum(),
                shown: shown.iter().any(|s| library::same_path(s, dir)),
            });
            return; // its subfolders are part of it
        }
    }
    if depth < MAX_DEPTH {
        for (p, _) in subdirs {
            walk(&p, depth + 1, root, avoid, shown, budget, out);
        }
    }
}

/// Every drive Windows has a letter for (C:\, D:\, an external E:\…).
pub fn drive_roots() -> Vec<PathBuf> {
    (b'A'..=b'Z').map(|c| PathBuf::from(format!("{}:\\", c as char))).filter(|p| p.is_dir()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn finds_medal_and_recordings_folders_on_a_drive() {
        let d = std::env::temp_dir().join(format!("hindsight-drives-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        put(&d.join("Old PC/Medal/Clips/Valorant/a.mp4"));
        put(&d.join("Old PC/Medal/Clips/Rust/b.mp4"));
        put(&d.join("recording/2024-01-01 10-00-00.mkv"));
        for i in 0..5 {
            put(&d.join(format!("Stuff/Gameplay dump/v{i}.mp4")));
        }
        put(&d.join("Stuff/holiday.mp4")); // one stray video: not a clip folder
        put(&d.join("Windows/Recordings/x.mp4")); // system folder: skipped
        put(&d.join("Hindsight/VALORANT/c.mkv")); // Hindsight's own: avoided
        put(&d.join("Shown/Clips/d.mp4"));
        let found = find(&[d.clone()], &[d.join("Hindsight")], &[d.join("Shown").join("Clips")]);
        let by = |end: &str| found.iter().find(|f| f.path.replace('\\', "/").ends_with(end));
        let medal = by("Old PC/Medal").expect("medal");
        assert_eq!((medal.label.as_str(), medal.clips), ("Medal", 2));
        let rec = by("/recording").expect("recording");
        assert_eq!((rec.label.as_str(), rec.clips), ("recording", 1));
        assert_eq!(by("Gameplay dump").expect("dump").clips, 5);
        assert!(by("/Stuff").is_none());
        let rel = |f: &FoundFolder| f.path[d.to_string_lossy().len()..].replace('\\', "/");
        assert!(found.iter().all(|f| !rel(f).starts_with("/Windows") && !rel(f).starts_with("/Hindsight")), "{found:?}");
        assert!(by("Shown/Clips").expect("shown").shown);
        let _ = fs::remove_dir_all(&d);
    }

    // Windows paths (drive letters, backslashes); Linux and macOS: phase 2 of docs/platforms-plan.md.
    #[cfg(windows)]
    #[test]
    fn labels() {
        assert_eq!(label_for(Path::new(r"E:\Videos\ShadowPlay")), "NVIDIA");
        assert_eq!(label_for(Path::new(r"E:\OBS\Recordings")), "OBS");
        assert_eq!(label_for(Path::new(r"E:\recording")), "recording");
    }
}
