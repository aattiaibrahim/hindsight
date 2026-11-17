//! One game, several folder spellings ("VALORANT" here, "Valorant" on the NAS,
//! "Tom Clancys Rainbow Six Siege" and "Tom Clancy's Rainbow Six  Siege").
//!
//! - `find_similar` spots them, across this PC's clips folder and the NAS, using
//!   the same rule as the library's game chips (case, spaces and punctuation
//!   don't count). Nothing is touched; the app asks first.
//! - `plan` works out the merge into the name the user picks, on both sides:
//!   a folder that has no rival on its side is simply renamed; otherwise its
//!   clips move into the chosen folder, never overwriting anything.
//! - The moves go through `consolidate::run`, so every one is logged and the
//!   whole merge can be undone like "Move into Hindsight".

use crate::consolidate::{game_key, Move, Plan};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Spelling {
    pub name: String,
    /// Clips in this folder on this PC
    pub here: usize,
    /// Clips in this folder on the NAS
    pub nas: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Similar {
    pub key: String,
    /// Most clips first
    pub names: Vec<Spelling>,
    /// The spelling with the most clips
    pub suggested: String,
}

/// The game a folder is for: "SpeciesUnknown-Win64-Shipping" counts as "Species Unknown".
pub fn folder_key(name: &str) -> String {
    game_key(&crate::games::tidy_program_name(name).unwrap_or_else(|| name.to_string()))
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Clips and bytes in `dir`, its subfolders included (up to three deep).
fn videos_under(dir: &Path, depth: u32) -> (usize, u64) {
    let Ok(r) = fs::read_dir(dir) else { return (0, 0) };
    let mut out = (0, 0);
    for f in r.flatten() {
        if hidden(&f.file_name().to_string_lossy()) {
            continue;
        }
        let p = f.path();
        if p.is_dir() {
            if depth < 3 {
                let (n, b) = videos_under(&p, depth + 1);
                out = (out.0 + n, out.1 + b);
            }
        } else if crate::library::is_video(&p) {
            out = (out.0 + 1, out.1 + fs::metadata(&p).map(|m| m.len()).unwrap_or(0));
        }
    }
    out
}

/// Game folders at the top of `root` with how many clips each holds, subfolders
/// included (a folder whose clips are all in "Game/nate moaning/" isn't empty).
pub fn folder_counts(root: &Path) -> Vec<(String, usize)> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !hidden(n))
        .map(|n| {
            let count = videos_under(&root.join(&n), 1).0;
            (n, count)
        })
        .collect()
}

/// Moves that put everything in `from` into `dest`: files (renamed if the name is
/// taken), and subfolders whole (or merged into one of the same name already there).
/// Before, subfolders were left behind, and the old folder with them.
fn move_contents(from: &Path, dest: &Path, moves: &mut Vec<Move>, depth: u32) {
    let Ok(r) = fs::read_dir(from) else { return };
    let mut taken: HashSet<String> = HashSet::new();
    for f in r.flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        let p = f.path();
        if p.is_file() {
            let to = free_name(dest, &name, &mut taken);
            let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            moves.push(Move { from: p, to, size, copied: false });
        } else if p.is_dir() {
            let to = dest.join(&name);
            if to.exists() && depth < 3 {
                move_contents(&p, &to, moves, depth + 1);
            } else if !to.exists() {
                moves.push(Move { from: p, to, size: 0, copied: false });
            }
        }
    }
}

/// Games that sit under more than one spelling. `dismissed` holds keys the user
/// chose to keep separate.
pub fn find_similar(here: &[(String, usize)], nas: &[(String, usize)], dismissed: &[String]) -> Vec<Similar> {
    let mut by_key: BTreeMap<String, BTreeMap<String, Spelling>> = BTreeMap::new();
    let mut add = |name: &str, n: usize, on_nas: bool| {
        let key = folder_key(name);
        if key.is_empty() {
            return;
        }
        let s = by_key
            .entry(key)
            .or_default()
            .entry(name.to_string())
            .or_insert_with(|| Spelling { name: name.to_string(), here: 0, nas: 0 });
        if on_nas {
            s.nas += n;
        } else {
            s.here += n;
        }
    };
    for (name, n) in here {
        add(name, *n, false);
    }
    for (name, n) in nas {
        add(name, *n, true);
    }
    by_key
        .into_iter()
        // Several spellings, or one named after the game's program file.
        .filter(|(k, names)| {
            (names.len() > 1 || names.keys().any(|n| crate::games::tidy_program_name(n).is_some())) && !dismissed.iter().any(|d| d == k)
        })
        .map(|(key, names)| {
            let mut names: Vec<Spelling> = names.into_values().collect();
            names.sort_by(|a, b| (b.here + b.nas).cmp(&(a.here + a.nas)).then_with(|| a.name.cmp(&b.name)));
            let mut suggested = names[0].name.clone();
            // "SpeciesUnknown-Win64-Shipping": offer the tidy name as the folder to keep.
            if let Some(tidy) = names.iter().find_map(|n| crate::games::tidy_program_name(&n.name)) {
                if !names.iter().any(|n| n.name == tidy) {
                    names.insert(0, Spelling { name: tidy.clone(), here: 0, nas: 0 });
                }
                suggested = tidy;
            }
            Similar { key, suggested, names }
        })
        .collect()
}

/// The moves that put every spelling of `key` under `target`, in each root.
/// Errors (and touches nothing) while a copy is still landing in one of them.
pub fn plan(roots: &[PathBuf], key: &str, target: &str) -> Result<Plan, String> {
    let mut plan = Plan::default();
    let mut labels: Vec<String> = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else { continue };
        let names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !hidden(n) && folder_key(n) == key)
            .collect();
        let others: Vec<&String> = names.iter().filter(|n| n.as_str() != target).collect();
        if others.is_empty() {
            continue;
        }
        let dest = root.join(target);
        let target_here = names.iter().any(|n| n == target);
        for name in &others {
            let from = root.join(name);
            let files: Vec<PathBuf> = fs::read_dir(&from)
                .map(|r| r.flatten().map(|f| f.path()).collect())
                .unwrap_or_default();
            if files.iter().any(|f| f.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".rewind-partial"))) {
                return Err(format!("A clip is still being copied into \"{name}\". Try again in a minute."));
            }
            let (clips, bytes) = videos_under(&from, 1);
            if !labels.contains(name) {
                labels.push((*name).clone());
            }
            plan.count += clips;
            plan.bytes += bytes;
            if !target_here && others.len() == 1 {
                // No rival on this side: one rename does it (instant, also on the NAS).
                plan.moves.push(Move { from, to: dest.clone(), size: 0, copied: false });
                continue;
            }
            move_contents(&from, &dest, &mut plan.moves, 1);
        }
    }
    plan.label = labels.join(", ");
    plan.folders = vec![(target.to_string(), plan.count)];
    Ok(plan)
}

/// `dir/name`, or `dir/stem (2).ext` and so on if that's taken.
fn free_name(dir: &Path, name: &str, taken: &mut HashSet<String>) -> PathBuf {
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut to = dir.join(name);
    let mut n = 2;
    while to.exists() || taken.contains(&to.to_string_lossy().to_lowercase()) {
        to = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    taken.insert(to.to_string_lossy().to_lowercase());
    to
}

/// After the files moved out: remove the old spellings' folders if they're empty.
pub fn remove_empty(roots: &[PathBuf], key: &str, target: &str) {
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name != target && !hidden(&name) && folder_key(&name) == key && e.path().is_dir() {
                remove_empty_tree(&e.path());
            }
        }
    }
}

/// Remove `dir` and the folders in it, if nothing but empty folders is left.
fn remove_empty_tree(dir: &Path) {
    if let Ok(r) = fs::read_dir(dir) {
        for f in r.flatten() {
            if f.path().is_dir() {
                remove_empty_tree(&f.path());
            }
        }
    }
    let _ = fs::remove_dir(dir); // only succeeds when empty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidate;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hindsight-merge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; bytes]).unwrap();
    }

    fn names(root: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn program_named_folders_get_a_tidy_name() {
        let here = vec![("SpeciesUnknown-Win64-Shipping".to_string(), 4), ("VALORANT".to_string(), 9)];
        let found = find_similar(&here, &[], &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].suggested, "Species Unknown");
        assert_eq!(found[0].names[0].name, "Species Unknown");
        assert_eq!(found[0].names[1].name, "SpeciesUnknown-Win64-Shipping");
    }

    #[test]
    fn program_named_folder_merges_into_its_tidy_name() {
        let pc = tmp("tidy-pc");
        let nas = tmp("tidy-nas");
        put(&pc.join("SpeciesUnknown-Win64-Shipping").join("a.mp4"), 10);
        put(&nas.join("SpeciesUnknown-Win64-Shipping").join("b.mp4"), 10);
        put(&nas.join("Species Unknown").join("c.mp4"), 10);
        let found = find_similar(&folder_counts(&pc), &folder_counts(&nas), &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        let s = &found[0];
        assert_eq!(s.suggested, "Species Unknown");
        // The name the dialog offers is accepted for this game.
        assert_eq!(folder_key(&s.suggested), s.key);
        let plan = plan(&[pc.clone(), nas.clone()], &s.key, &s.suggested).unwrap();
        let log = pc.join("merge.jsonl");
        let out = consolidate::run(&plan, &log, |_, _| {});
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        remove_empty(&[pc.clone(), nas.clone()], &s.key, &s.suggested);
        assert!(pc.join("Species Unknown").join("a.mp4").exists());
        assert!(nas.join("Species Unknown").join("b.mp4").exists());
        assert!(nas.join("Species Unknown").join("c.mp4").exists());
        assert!(!pc.join("SpeciesUnknown-Win64-Shipping").exists());
        assert!(!nas.join("SpeciesUnknown-Win64-Shipping").exists());
        let _ = fs::remove_dir_all(&pc);
        let _ = fs::remove_dir_all(&nas);
    }

    #[test]
    fn finds_spellings_across_this_pc_and_the_nas() {
        let here = vec![
            ("VALORANT".to_string(), 91),
            ("Tom Clancys Rainbow Six Siege".to_string(), 27),
            ("Desktop".to_string(), 34),
        ];
        let nas = vec![
            ("Valorant".to_string(), 6),
            ("Tom Clancy's Rainbow Six  Siege".to_string(), 29),
            ("Desktop".to_string(), 34),
            ("Rust".to_string(), 3),
        ];
        let found = find_similar(&here, &nas, &[]);
        assert_eq!(found.len(), 2, "{found:?}");
        let r6 = found.iter().find(|s| s.key == "tomclancysrainbowsixsiege").unwrap();
        assert_eq!(r6.suggested, "Tom Clancy's Rainbow Six  Siege");
        assert_eq!(r6.names[1], Spelling { name: "Tom Clancys Rainbow Six Siege".into(), here: 27, nas: 0 });
        let val = found.iter().find(|s| s.key == "valorant").unwrap();
        assert_eq!(val.suggested, "VALORANT");
        // Same name on both sides is one spelling, not a clash.
        assert!(!found.iter().any(|s| s.key == "desktop"));
        // "Keep separate" sticks.
        assert_eq!(find_similar(&here, &nas, &["valorant".into()]).len(), 1);
    }

    #[test]
    fn merge_renames_or_moves_and_undo_puts_it_all_back() {
        let pc = tmp("pc");
        let nas = tmp("nas");
        put(&pc.join("Tom Clancys Rainbow Six Siege/a.mkv"), 10);
        put(&pc.join("Tom Clancys Rainbow Six Siege/b.mkv"), 11);
        put(&nas.join("Tom Clancy's Rainbow Six  Siege/a.mkv"), 12); // same name, different clip
        put(&nas.join("Tom Clancy's Rainbow Six  Siege/c.mov"), 13);
        put(&nas.join("Tom Clancys Rainbow Six Siege/b.mkv"), 11); // already copied by the sync
        let roots = vec![pc.clone(), nas.clone()];
        let target = "Tom Clancy's Rainbow Six  Siege";
        let plan = plan(&roots, "tomclancysrainbowsixsiege", target).unwrap();
        assert_eq!(plan.label, "Tom Clancys Rainbow Six Siege");

        let log = pc.join("merge.jsonl");
        let out = consolidate::run(&plan, &log, |_, _| {});
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        remove_empty(&roots, "tomclancysrainbowsixsiege", target);

        // This PC had no rival folder: renamed as a whole.
        assert_eq!(names(&pc), vec!["Tom Clancy's Rainbow Six  Siege", "merge.jsonl"]);
        assert_eq!(names(&pc.join(target)), vec!["a.mkv", "b.mkv"]);
        // The NAS had both: clips moved in, nothing overwritten, old folder gone.
        assert_eq!(names(&nas), vec!["Tom Clancy's Rainbow Six  Siege"]);
        assert_eq!(names(&nas.join(target)), vec!["a.mkv", "b.mkv", "c.mov"]);
        assert_eq!(fs::read(nas.join(target).join("a.mkv")).unwrap().len(), 12);

        let back = consolidate::undo(&log);
        assert!(back.failed.is_empty(), "{:?}", back.failed);
        assert_eq!(names(&pc.join("Tom Clancys Rainbow Six Siege")), vec!["a.mkv", "b.mkv"]);
        assert_eq!(names(&nas.join("Tom Clancys Rainbow Six Siege")), vec!["b.mkv"]);
        assert_eq!(names(&nas.join(target)), vec!["a.mkv", "c.mov"]);
        let _ = fs::remove_dir_all(&pc);
        let _ = fs::remove_dir_all(&nas);
    }

    #[test]
    fn a_folder_of_clips_inside_a_spelling_is_counted_and_moved_too() {
        let root = tmp("nested");
        put(&root.join("Tom Clancys Rainbow Six Siege").join("a.mp4"), 10);
        put(&root.join("Tom Clancy's Rainbow Six  Siege").join("nate moaning").join("b.mp4"), 20);
        put(&root.join("Tom Clancy's Rainbow Six  Siege").join("nate moaning").join("c.mp4"), 20);
        let counts = folder_counts(&root);
        assert!(counts.contains(&("Tom Clancy's Rainbow Six  Siege".into(), 2)), "{counts:?}");
        let found = find_similar(&counts, &[], &[]);
        assert_eq!(found.len(), 1);
        let p = plan(std::slice::from_ref(&root), &found[0].key, "Tom Clancys Rainbow Six Siege").unwrap();
        assert_eq!(p.count, 2);
        let _ = consolidate::run(&p, &tmp("nested-log").join("log.txt"), |_, _| {});
        remove_empty(std::slice::from_ref(&root), &found[0].key, "Tom Clancys Rainbow Six Siege");
        let kept = root.join("Tom Clancys Rainbow Six Siege");
        assert!(kept.join("nate moaning").join("b.mp4").is_file() && kept.join("a.mp4").is_file());
        assert!(!root.join("Tom Clancy's Rainbow Six  Siege").exists(), "the old spelling's folder is gone");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn case_only_spellings_are_renamed_in_place() {
        let nas = tmp("case");
        put(&nas.join("Valorant/x.mkv"), 5);
        let roots = vec![nas.clone()];
        let plan = plan(&roots, "valorant", "VALORANT").unwrap();
        assert_eq!(plan.moves.len(), 1);
        let log = nas.join("..").join(format!("case-{}.jsonl", std::process::id()));
        let out = consolidate::run(&plan, &log, |_, _| {});
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        assert_eq!(names(&nas), vec!["VALORANT"]);
        assert_eq!(names(&nas.join("VALORANT")), vec!["x.mkv"]);
        assert!(consolidate::undo(&log).failed.is_empty());
        assert_eq!(names(&nas), vec!["Valorant"]);
        let _ = fs::remove_dir_all(&nas);
    }

    #[test]
    fn waits_while_a_copy_is_landing() {
        let nas = tmp("partial");
        put(&nas.join("Valorant/.x.mkv.rewind-partial"), 5);
        put(&nas.join("VALORANT_/y.mkv"), 5);
        let err = plan(&[nas.clone()], "valorant", "VALORANT_").unwrap_err();
        assert!(err.contains("still being copied"), "{err}");
        let _ = fs::remove_dir_all(&nas);
    }
}
