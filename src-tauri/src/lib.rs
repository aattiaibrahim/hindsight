mod app_audio;
mod prepare;
mod game_icons;
mod duplicates;
mod montage;
mod game_detect;
#[macro_use]
mod diag;
mod audio;
mod audio_edit;
mod bench;
mod consolidate;
mod control;
mod drives;
mod engine;
mod ffmpeg;
mod folder_merge;
mod games;
mod health;
mod library;
mod monitors;
mod media_server;
mod nas;
mod old_folders;
mod native;
mod secrets;
mod settings;
mod sound;
mod updater;
mod upload;

use settings::Settings;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub struct AppState {
    pub(crate) settings: Arc<Mutex<Settings>>,
    settings_path: PathBuf,
    pub(crate) engine: engine::Engine,
    thumbs_dir: PathBuf,
    flyout_hidden_at: Mutex<Instant>,
}

fn settings_of(app: &AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
}

/// Clips folder/<Game or Desktop>/<Game or Desktop> <label><timestamp>.mkv
/// With a shared NAS, the PC's name is added (" - GAMING-PC") so clips from
/// several PCs in the same game folder never clash.
fn output_path(s: &Settings, label: &str) -> PathBuf {
    let folder = games::clip_folder();
    // A spelling picked when merging this game's folders wins, so they don't split again.
    let folder = s.folder_names.get(&consolidate::game_key(&folder)).cloned().unwrap_or(folder);
    let ts = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let pc = if s.nas_dir.is_empty() { String::new() } else { nas::pc_suffix() };
    let dir = PathBuf::from(&s.clips_dir).join(&folder);
    let taken = |p: &PathBuf| p.exists() || nas::exists_on_nas(s, p);
    let mut p = dir.join(format!("{folder} {label}{ts}{pc}.mkv"));
    let mut n = 2;
    while taken(&p) {
        p = dir.join(format!("{folder} {label}{ts}{pc} ({n}).mkv"));
        n += 1;
    }
    p
}

fn save_now(app: &AppHandle, seconds: Option<u32>) -> Result<String, String> {
    let s = settings_of(app);
    let out = output_path(&s, "");
    let saved = app.state::<AppState>().engine.save_clip(seconds.unwrap_or(s.buffer_seconds), &out);
    if s.save_sound {
        if saved.is_ok() { sound::saved() } else { sound::failed() }
    }
    saved?;
    let path = out.to_string_lossy().into_owned();
    let _ = app.emit("clip-saved", &path);
    if !s.nas_dir.is_empty() {
        nas::clip_saved();
    }
    if s.auto_upload {
        // In the background; the link is copied and a toast shows when it's done.
        let (a, p) = (app.clone(), path.clone());
        thread::spawn(move || {
            let _ = upload_now(&a, &p, None);
        });
    }
    Ok(path)
}

/// Start or stop a recording. Returns the saved file when stopping.
fn toggle_record_now(app: &AppHandle) -> Result<Option<String>, String> {
    let st = app.state::<AppState>();
    if st.engine.is_recording() {
        let stopped = st.engine.stop_recording();
        if settings_of(app).save_sound {
            if stopped.is_ok() { sound::saved() } else { sound::failed() }
        }
        let files = stopped?;
        nas::clip_saved();
        for f in &files {
            let _ = app.emit("clip-saved", f);
        }
        let _ = app.emit("recording-changed", false);
        Ok(files.into_iter().next())
    } else {
        let out = output_path(&settings_of(app), "Recording ");
        st.engine.start_recording(&out)?;
        let _ = app.emit("recording-changed", true);
        Ok(None)
    }
}

fn in_background(app: &AppHandle, job: fn(&AppHandle) -> Result<(), String>) {
    let app = app.clone();
    thread::spawn(move || {
        if let Err(e) = job(&app) {
            let _ = app.emit("clip-error", e);
        }
    });
}

/// Register both hotkeys; on failure the previous ones are restored.
fn apply_hotkeys(app: &AppHandle, save: &str, record: &str) -> Result<(), String> {
    if Shortcut::from_str(save).ok() == Shortcut::from_str(record).ok() {
        return Err("The clip and record hotkeys can't be the same.".into());
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    gs.register(save).map_err(|e| format!("Couldn't use hotkey \"{save}\": {e}"))?;
    gs.register(record).map_err(|e| format!("Couldn't use hotkey \"{record}\": {e}"))?;
    Ok(())
}

/// The app icon, with a red "live" dot in the corner while capturing (like OBS).
fn tray_icon(base: &tauri::image::Image<'_>, live: bool) -> tauri::image::Image<'static> {
    let (w, h) = (base.width(), base.height());
    let mut px = base.rgba().to_vec();
    if live {
        let size = w.min(h) as f32;
        let r = size * 0.16; // dot radius
        let ring = size * 0.05; // dark outline so it reads on any taskbar
        let (cx, cy) = (w as f32 - r - ring, h as f32 - r - ring);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let color = if d <= r {
                    Some([0xFF, 0x3B, 0x30])
                } else if d <= r + ring {
                    Some([0x0E, 0x10, 0x14])
                } else {
                    None
                };
                if let Some([cr, cg, cb]) = color {
                    // Soft 1px edge
                    let edge = (r + ring - d).clamp(0.0, 1.0);
                    let i = ((y * w + x) * 4) as usize;
                    let a = edge;
                    px[i] = (cr as f32 * a + px[i] as f32 * (1.0 - a)) as u8;
                    px[i + 1] = (cg as f32 * a + px[i + 1] as f32 * (1.0 - a)) as u8;
                    px[i + 2] = (cb as f32 * a + px[i + 2] as f32 * (1.0 - a)) as u8;
                    px[i + 3] = (255.0 * a + px[i + 3] as f32 * (1.0 - a)) as u8;
                }
            }
        }
    }
    tauri::image::Image::new_owned(px, w, h)
}

fn apply_autostart(app: &AppHandle, on: bool) {
    let a = app.autolaunch();
    let _ = if on { a.enable() } else { a.disable() };
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn toggle_flyout(app: &AppHandle, at: PhysicalPosition<f64>) {
    let Some(w) = app.get_webview_window("flyout") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    // A click on the tray icon blurs (and hides) the flyout first; don't reopen it.
    if app.state::<AppState>().flyout_hidden_at.lock().unwrap().elapsed() < Duration::from_millis(300) {
        return;
    }
    let size = w.outer_size().unwrap_or_default();
    let (w_, h_) = (size.width as f64, size.height as f64);
    let x = (at.x - w_ + 24.0).max(0.0);
    let y = if at.y > h_ { at.y - h_ - 16.0 } else { at.y + 16.0 };
    let _ = w.set_position(PhysicalPosition::new(x, y));
    let _ = w.show();
    let _ = w.set_focus();
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

// ---------- commands ----------

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    settings_of(&app)
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let settings = settings.sanitized();
    audio::set_mic_gain_db(settings.mic_gain_db);
    let st = app.state::<AppState>();
    let old = st.settings.lock().unwrap().clone();
    if old.hotkey != settings.hotkey || old.record_hotkey != settings.record_hotkey {
        if let Err(e) = apply_hotkeys(&app, &settings.hotkey, &settings.record_hotkey) {
            let _ = apply_hotkeys(&app, &old.hotkey, &old.record_hotkey);
            return Err(e);
        }
    }
    if old.clips_dir != settings.clips_dir {
        std::fs::create_dir_all(&settings.clips_dir)
            .map_err(|e| format!("Can't use that clips folder: {e}"))?;
    }
    // A new NAS folder must have been checked and confirmed first (see `nas_confirm`).
    if !settings.nas_dir.is_empty() && settings.nas_dir != old.nas_dir {
        let marker = nas::read_marker(std::path::Path::new(&settings.nas_dir));
        if settings.nas_library_id.is_empty() || marker.as_deref() != Some(settings.nas_library_id.as_str()) {
            return Err("Check the NAS folder first: choose it in Settings > Library.".into());
        }
    }
    if !settings.nas_dir.is_empty() {
        let (n, c) = (settings.nas_dir.to_lowercase(), settings.clips_dir.trim_end_matches(['\\', '/']).to_lowercase());
        if n == c || n.starts_with(&format!("{c}\\")) || c.starts_with(&format!("{n}\\")) {
            return Err("The NAS folder has to be separate from the clips folder on this PC.".into());
        }
    }
    settings::save(&st.settings_path, &settings)?;
    *st.settings.lock().unwrap() = settings.clone();
    library::set_twin(&settings.nas_dir, &settings.clips_dir);
    if old.launch_on_startup != settings.launch_on_startup {
        apply_autostart(&app, settings.launch_on_startup);
    }
    if old.nas_dir != settings.nas_dir || old.nas_remove_local != settings.nas_remove_local || old.clips_dir != settings.clips_dir {
        nas::wake();
    }
    if old.linked_folders != settings.linked_folders || old.nas_dir != settings.nas_dir {
        let _ = app.emit("clips-changed", ());
    }
    let _ = app.emit("settings-changed", &settings);
    Ok(())
}

#[tauri::command]
fn get_status(app: AppHandle) -> engine::Status {
    app.state::<AppState>().engine.status()
}

#[tauri::command]
fn restart_capture(app: AppHandle) {
    app.state::<AppState>().engine.restart();
}

#[tauri::command]

async fn nas_check(app: AppHandle, dir: String) -> nas::FolderCheck {
    let s = settings_of(&app);
    tauri::async_runtime::spawn_blocking(move || nas::check_folder(std::path::Path::new(&s.clips_dir), std::path::Path::new(&dir)))
        .await
        .unwrap_or_default()
}

/// Mark the checked folder as the library and return its id (for `nasLibraryId`).
#[tauri::command]
async fn nas_confirm(dir: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || nas::confirm_folder(std::path::Path::new(&dir))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn nas_pair(dir: String, server: String, code: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || nas::pair(std::path::Path::new(&dir), &server, &code)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_monitors() -> Vec<monitors::Monitor> {
    monitors::list()
}

/// A live picture of the screen that gets recorded, for picking the recording
/// area in Settings. Refused while a game runs, so it never costs game FPS.
#[tauri::command]
async fn screen_preview(app: AppHandle) -> Result<tauri::ipc::Response, String> {
    let s = settings_of(&app);
    tauri::async_runtime::spawn_blocking(move || {
        if games::game_running() {
            return Err("game".to_string());
        }
        let mons = monitors::list();
        let m = monitors::pick(&s.monitor_mode, &s.fixed_monitor, &mons).ok_or("No screen found")?;
        monitors::preview_png(&m, 960).map(tauri::ipc::Response::new)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_audio_inputs() -> Vec<String> {
    audio::input_devices()
}

#[tauri::command]
async fn save_clip(app: AppHandle, seconds: Option<u32>) -> Result<String, String> {
    blocking(move || save_now(&app, seconds)).await
}

#[tauri::command]
async fn toggle_recording(app: AppHandle) -> Result<Option<String>, String> {
    blocking(move || toggle_record_now(&app)).await
}

#[tauri::command]
async fn list_clips(app: AppHandle) -> Result<Vec<library::ClipFile>, String> {
    let s = settings_of(&app);
    blocking(move || {
        let nas = nas::listing(&s.nas_dir);
        let linked = linked_roots(&s);
        Ok(library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &nas, &linked, &s.nas_dir))
    })
    .await
}

/// Linked folders with a label: the recorder's name if it's that recorder's folder.
fn linked_roots(s: &Settings) -> Vec<library::LinkedRoot> {
    if s.linked_folders.is_empty() {
        return Vec::new();
    }
    let known = bench::recorders::clip_folders();
    s.linked_folders
        .iter()
        .map(|p| library::LinkedRoot {
            path: PathBuf::from(p),
            label: known
                .iter()
                .find(|k| k.path.eq_ignore_ascii_case(p.trim_end_matches(['\\', '/'])))
                .map(|k| k.label.clone())
                .unwrap_or_else(|| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone())),
        })
        .collect()
}

#[tauri::command]
fn get_nas_state() -> nas::NasState {
    nas::state()
}

#[tauri::command]
fn sync_nas_now() {
    nas::wake();
}

/// Stop (or resume) NAS copying and freeing up, from the status bar.
#[tauri::command]
fn stop_nas_sync(stop: bool) {
    nas::set_stopped(stop);
}

fn linked_root_for(s: &Settings, folder: &str) -> Result<library::LinkedRoot, String> {
    linked_roots(s)
        .into_iter()
        .find(|r| library::same_path(&r.path, std::path::Path::new(folder)))
        .ok_or_else(|| "That folder isn't shown in your library.".to_string())
}

fn moves_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map(|d| d.join("moves")).map_err(|e| e.to_string())
}

/// Folders a move must never reach into: Hindsight's own, the NAS, and the linked
/// folders (each root being moved skips only the others).
fn move_avoid(s: &Settings) -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from(&s.clips_dir)];
    if !s.nas_dir.is_empty() {
        v.push(PathBuf::from(&s.nas_dir));
    }
    v.extend(s.linked_folders.iter().map(PathBuf::from));
    v
}

fn move_plan(s: &Settings, folders: &[String]) -> Result<consolidate::Plan, String> {
    let roots = folders.iter().map(|f| linked_root_for(s, f)).collect::<Result<Vec<_>, _>>()?;
    if roots.is_empty() {
        return Err("No folders to move.".into());
    }
    Ok(consolidate::plan_many(&roots, std::path::Path::new(&s.clips_dir), &move_avoid(s)))
}

/// What "Move into Hindsight" would do for one or more linked folders (nothing is touched).
#[tauri::command]
async fn plan_move_into_rewind(app: AppHandle, folders: Vec<String>) -> Result<consolidate::Plan, String> {
    let s = settings_of(&app);
    blocking(move || move_plan(&s, &folders)).await
}

#[tauri::command]
async fn move_into_rewind(app: AppHandle, folders: Vec<String>) -> Result<consolidate::Outcome, String> {
    let s = settings_of(&app);
    let dir = moves_dir(&app)?;
    let a = app.clone();
    let out = blocking(move || {
        let plan = move_plan(&s, &folders)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        // "Medal, NVIDIA" → "Medal+NVIDIA" in the file name (shown again as "Medal, NVIDIA").
        let tag = plan.label.replace(", ", "+").replace(|c: char| !c.is_ascii_alphanumeric() && c != '+', "_");
        let log = dir.join(format!("{stamp}-{tag}.jsonl"));
        Ok(consolidate::run(&plan, &log, |done, total| {
            let _ = a.emit("move-progress", (done, total));
        }))
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    nas::wake();
    Ok(out)
}

/// Hindsight's clips folder and the NAS: never offered as an old folder.
fn own_folders(s: &Settings) -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from(&s.clips_dir)];
    if !s.nas_dir.is_empty() {
        v.push(PathBuf::from(&s.nas_dir));
    }
    v
}

/// Recorder folders that "Move into Hindsight" emptied (no clips left in them).
#[tauri::command]
async fn old_folders(app: AppHandle) -> Result<Vec<old_folders::OldFolder>, String> {
    let s = settings_of(&app);
    let logs = moves_dir(&app)?;
    blocking(move || Ok(old_folders::find(&old_folder_candidates(&s), &own_folders(&s), &logs))).await
}

/// Folders that might be emptied recorder folders: linked ones and the recorders' own.
fn old_folder_candidates(s: &Settings) -> Vec<String> {
    let mut v = s.linked_folders.clone();
    v.extend(bench::recorders::clip_folders().into_iter().filter(|f| f.exists).map(|f| f.path));
    v
}

/// Send an emptied recorder folder to the Recycle Bin and stop showing it.
#[tauri::command]
async fn remove_old_folder(app: AppHandle, path: String) -> Result<(), String> {
    let s = settings_of(&app);
    let logs = moves_dir(&app)?;
    let (p, linked, keep) = (path.clone(), old_folder_candidates(&s), own_folders(&s));
    blocking(move || old_folders::remove(&p, &linked, &keep, &logs)).await?;
    let mut s = settings_of(&app);
    s.linked_folders.retain(|f| !library::same_path(std::path::Path::new(f), std::path::Path::new(&path)));
    set_settings(app.clone(), s)?;
    let _ = app.emit("clips-changed", ());
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UndoInfo {
    label: String,
    count: usize,
    /// ms since 1970
    when: i64,
}

fn latest_move_log(app: &AppHandle) -> Option<PathBuf> {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(moves_dir(app).ok()?)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    logs.sort();
    logs.pop()
}

/// The most recent move that can still be undone, if any.
#[tauri::command]
fn last_move(app: AppHandle) -> Option<UndoInfo> {
    let log = latest_move_log(&app)?;
    let count = consolidate::read_log(&log).len();
    if count == 0 {
        return None;
    }
    let name = log.file_stem()?.to_string_lossy().into_owned();
    let label = name.splitn(3, '-').nth(2).unwrap_or("folder").replace('_', " ").replace('+', ", ");
    let when = std::fs::metadata(&log)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(UndoInfo { label, count, when })
}

#[tauri::command]
async fn undo_last_move(app: AppHandle) -> Result<consolidate::Outcome, String> {
    let log = latest_move_log(&app).ok_or("Nothing to undo.")?;
    let out = blocking(move || Ok(consolidate::undo(&log))).await?;
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

/// Cleanup's Shrink: compress to about 40% of the size, replace the original (it goes to the
/// Recycle Bin), and keep the clip's date so it stays where it was in the library.
/// Cleanup: remove this PC's copy of a clip that's safely (byte-identical) on the NAS.
#[tauri::command]
async fn free_local_copy(app: AppHandle, path: String) -> Result<(), String> {
    let s = settings_of(&app);
    blocking(move || nas::free_local(&s, std::path::Path::new(&path))).await?;
    let _ = app.emit("clips-changed", ());
    Ok(())
}

#[tauri::command]
async fn shrink_clip(app: AppHandle, path: String) -> Result<String, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let when = meta.modified().ok();
    let target_mb = ((meta.len() as f64 / 1_048_576.0) * 0.4).round().max(8.0) as u32;
    if (target_mb as u64) * 1_048_576 >= meta.len() {
        return Err("It's already small; shrinking wouldn't save anything.".into());
    }
    let secs = duration_of(&path);
    let out = edit(app.clone(), path, true, secs, move |p| library::compress(p, target_mb, &Default::default(), None)).await?;
    if let Some(t) = when {
        let _ = std::fs::File::options().write(true).open(&out).and_then(|f| f.set_modified(t));
    }
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

// ---------- Clips on other drives (an external HDD, an old Medal folder) ----------

/// Look through every drive for folders with clips. Read only; can take a few seconds.
#[tauri::command]
async fn find_clip_folders(app: AppHandle) -> Result<Vec<drives::FoundFolder>, String> {
    let s = settings_of(&app);
    blocking(move || {
        let mut avoid = vec![PathBuf::from(&s.clips_dir)];
        if !s.nas_dir.is_empty() {
            avoid.push(PathBuf::from(&s.nas_dir));
        }
        let shown: Vec<PathBuf> = s.linked_folders.iter().map(PathBuf::from).collect();
        Ok(drives::find(&drives::drive_roots(), &avoid, &shown))
    })
    .await
}

fn import_plan(s: &Settings, path: &str, copy: bool) -> Result<consolidate::Plan, String> {
    let p = PathBuf::from(path);
    if !p.is_dir() {
        return Err("That folder isn't there anymore (is the drive still connected?).".into());
    }
    let root = library::LinkedRoot { label: drives::label_for(&p), path: p };
    let avoid: Vec<PathBuf> = move_avoid(s).into_iter().filter(|a| !library::same_path(a, &root.path)).collect();
    let plan = consolidate::plan_many(std::slice::from_ref(&root), std::path::Path::new(&s.clips_dir), &avoid);
    Ok(if copy { consolidate::as_copy(plan) } else { plan })
}

/// What importing a folder would do: how many clips, how big, where they land. Nothing is touched.
#[tauri::command]
async fn plan_import(app: AppHandle, path: String, copy: bool) -> Result<consolidate::Plan, String> {
    let s = settings_of(&app);
    blocking(move || import_plan(&s, &path, copy)).await
}

/// Copy (originals stay) or move a folder's clips into Hindsight's game folders. Undo with "Undo last move".
#[tauri::command]
async fn import_folder(app: AppHandle, path: String, copy: bool) -> Result<consolidate::Outcome, String> {
    let s = settings_of(&app);
    let dir = moves_dir(&app)?;
    let a = app.clone();
    let out = blocking(move || {
        let plan = import_plan(&s, &path, copy)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let tag = format!("{}{}", plan.label, if copy { "+copied" } else { "" })
            .replace(|c: char| !c.is_ascii_alphanumeric() && c != '+', "_");
        let log = dir.join(format!("{stamp}-{tag}.jsonl"));
        Ok(consolidate::run(&plan, &log, |done, total| {
            let _ = a.emit("move-progress", (done, total));
        }))
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    nas::wake();
    Ok(out)
}

// ---------- Game folders under different spellings ----------

/// Change saved settings from the backend (same path as the Settings page).
fn change_settings(app: &AppHandle, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let st = app.state::<AppState>();
    let mut s = st.settings.lock().unwrap().clone();
    f(&mut s);
    let s = s.sanitized();
    settings::save(&st.settings_path, &s)?;
    *st.settings.lock().unwrap() = s.clone();
    let _ = app.emit("settings-changed", &s);
    Ok(())
}

/// Games saved under more than one folder spelling, on this PC and the NAS.
/// The NAS side comes from the last sync's listing, so this never waits on the network.
#[tauri::command]
async fn similar_game_folders(app: AppHandle) -> Result<Vec<folder_merge::Similar>, String> {
    let s = settings_of(&app);
    blocking(move || {
        let here = folder_merge::folder_counts(std::path::Path::new(&s.clips_dir));
        let mut nas: Vec<(String, usize)> = Vec::new();
        for f in nas::listing(&s.nas_dir) {
            match nas.iter_mut().find(|(n, _)| *n == f.folder) {
                Some((_, k)) => *k += 1,
                None => nas.push((f.folder.clone(), 1)),
            }
        }
        nas.retain(|(n, _)| n != "Other");
        Ok(folder_merge::find_similar(&here, &nas, &s.similar_dismissed))
    })
    .await
}

/// Put every spelling of one game under `target`, here and on the NAS. Undo with "Undo last move".
#[tauri::command]
async fn merge_game_folders(app: AppHandle, key: String, target: String) -> Result<consolidate::Outcome, String> {
    let s = settings_of(&app);
    let target = consolidate::safe_name(&target);
    if folder_merge::folder_key(&target) != key {
        return Err("That name is for a different game.".into());
    }
    let mut roots = vec![PathBuf::from(&s.clips_dir)];
    if !s.nas_dir.is_empty() {
        if nas::state().copying.is_some() {
            return Err("A clip is copying to the NAS right now. Try again once it's done.".into());
        }
        let nas_root = PathBuf::from(&s.nas_dir);
        let reachable = { let p = nas_root.clone(); blocking(move || Ok(p.is_dir())).await? };
        if !reachable {
            return Err("Can't reach the NAS right now. Merge once it's back, so both sides match.".into());
        }
        roots.push(nas_root);
    }
    let dir = moves_dir(&app)?;
    nas::hold(true);
    let (k, t) = (key.clone(), target.clone());
    let result = blocking(move || {
        let plan = folder_merge::plan(&roots, &k, &t)?;
        if plan.moves.is_empty() {
            return Ok((consolidate::Outcome::default(), Vec::new()));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let tag = plan.label.replace(", ", "+").replace(|c: char| !c.is_ascii_alphanumeric() && c != '+', "_");
        let log = dir.join(format!("{stamp}-{tag}.jsonl"));
        let out = consolidate::run(&plan, &log, |_, _| {});
        folder_merge::remove_empty(&roots, &k, &t);
        let renamed: Vec<String> = plan.label.split(", ").map(str::to_string).collect();
        Ok((out, renamed))
    })
    .await;
    if let Ok((_, renamed)) = &result {
        for from in renamed {
            nas::folder_renamed(from, &target);
        }
    }
    nas::hold(false);
    let (out, _) = result?;
    change_settings(&app, |s| {
        s.folder_names.insert(key.clone(), target.clone());
        s.similar_dismissed.retain(|d| *d != key);
    })?;
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

/// "Keep separate": stop asking about this game (or, with `ask_again`, ask about all again).
#[tauri::command]
fn keep_folders_separate(app: AppHandle, key: Option<String>) -> Result<(), String> {
    change_settings(&app, |s| match key {
        Some(k) => s.similar_dismissed.push(k),
        None => s.similar_dismissed.clear(),
    })
}

// ---------- Sharing (uploads) ----------

fn data_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    app.path().app_data_dir().map(|d| d.join(name)).map_err(|e| e.to_string())
}

/// Clips being uploaded right now (so one clip isn't sent twice at once).
static UPLOADING: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UploadProgress {
    path: String,
    host: String,
    /// "preparing" | "uploading" | "done" | "error"
    stage: &'static str,
    sent: u64,
    total: u64,
    url: Option<String>,
    error: Option<String>,
    /// e.g. "your network blocks files.catbox.moe"
    warning: Option<String>,
}

/// Prepare, upload, copy the link, remember it. Progress goes out as "upload-progress".
fn upload_now(app: &AppHandle, path: &str, host: Option<&str>) -> Result<upload::Upload, String> {
    let s = settings_of(app);
    let host = upload::Host::parse(host.unwrap_or(&s.upload_host)).ok_or("Unknown upload service.")?;
    {
        let mut busy = UPLOADING.lock().unwrap();
        if busy.iter().any(|p| p == path) {
            return Err("That clip is already uploading.".into());
        }
        busy.push(path.to_string());
    }
    let name = serde_json::to_value(host).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let emit = |stage, sent, total, url: Option<String>, error: Option<String>, warning: Option<String>| {
        let _ = app.emit("upload-progress", UploadProgress { path: path.to_string(), host: name.clone(), stage, sent, total, url, error, warning });
    };
    let result = (|| -> Result<upload::Upload, String> {
        emit("preparing", 0, 0, None, None, None);
        let work = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("uploads");
        let file = upload::prepare(path, host, &work)?;
        let secrets_file = data_file(app, "secrets.json")?;
        let folder = std::path::Path::new(path).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let cfg = upload::Config {
            litterbox_time: s.litterbox_time.clone(),
            discord_webhook: secrets::get(&secrets_file, "discord_webhook"),
            streamable_email: s.streamable_email.clone(),
            streamable_password: secrets::get(&secrets_file, "streamable_password"),
            post_link_to_discord: s.post_link_to_discord,
            message: if folder.is_empty() || folder == "Desktop" { "Clip".into() } else { format!("{folder} clip") },
        };
        let (a, p, h) = (app.clone(), path.to_string(), name.clone());
        let report = Box::new(move |sent: u64, total: u64| {
            let _ = a.emit("upload-progress", UploadProgress { path: p.clone(), host: h.clone(), stage: "uploading", sent, total, url: None, error: None, warning: None });
        });
        let sent = upload::send(host, &file, &cfg, &upload::Endpoints::default(), report);
        let bytes = fs_len(&file);
        if file != std::path::Path::new(path) {
            let _ = std::fs::remove_file(&file);
        }
        let url = sent?;
        let _ = upload::copy_to_clipboard(&url);
        let now = chrono::Local::now().timestamp_millis();
        let expires = match host {
            upload::Host::Litterbox => Some(upload::litterbox_expiry(&s.litterbox_time, now)),
            upload::Host::Discord => upload::discord_link_expiry(&url),
            _ => None,
        };
        let u = upload::Upload { host, url, at: now, bytes, expires };
        upload::remember(&data_file(app, "uploads.json")?, path, u.clone());
        Ok(u)
    })();
    UPLOADING.lock().unwrap().retain(|p| p != path);
    match &result {
        Ok(u) => emit("done", u.bytes, u.bytes, Some(u.url.clone()), None, upload::link_warning(&u.url)),
        Err(e) => emit("error", 0, 0, None, Some(e.clone()), None),
    }
    result
}

fn fs_len(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

#[tauri::command]
async fn upload_clip(app: AppHandle, path: String, host: Option<String>) -> Result<upload::Upload, String> {
    blocking(move || upload_now(&app, &path, host.as_deref())).await
}

/// Every clip's links (path → uploads, newest last).
#[tauri::command]
fn upload_history(app: AppHandle) -> Result<std::collections::BTreeMap<String, Vec<upload::Upload>>, String> {
    Ok(upload::history(&data_file(&app, "uploads.json")?))
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    upload::copy_to_clipboard(&text)
}

/// Store a secret (empty removes it). The value never comes back to the interface.
/// A Discord webhook is checked with Discord first, and only saved if it works.
#[tauri::command]
async fn set_secret(app: AppHandle, name: String, value: String) -> Result<Option<upload::WebhookInfo>, String> {
    let value = value.trim().to_string();
    blocking(move || {
        let file = data_file(&app, "secrets.json")?;
        let mut info = None;
        if name == "discord_webhook" {
            let mut s = settings_of(&app);
            if value.is_empty() {
                s.discord_webhook_name.clear();
            } else {
                let i = upload::check_webhook(&value)?;
                s.discord_webhook_name = i.name.clone();
                info = Some(i);
            }
            secrets::set(&file, &name, &value)?;
            set_settings(app.clone(), s)?;
        } else {
            secrets::set(&file, &name, &value)?;
        }
        Ok(info)
    })
    .await
}

/// Can this PC reach each upload service, and open its links? Uploads nothing.
#[tauri::command]
async fn check_sharing(app: AppHandle) -> Result<Vec<(upload::Host, Vec<upload::Reach>)>, String> {
    let hook = secrets::get(&data_file(&app, "secrets.json")?, "discord_webhook");
    blocking(move || {
        let mut out: Vec<(upload::Host, Vec<upload::Reach>)> = [upload::Host::Catbox, upload::Host::Litterbox, upload::Host::Discord, upload::Host::Streamable]
            .into_iter()
            .map(|h| (h, upload::needed_hosts(h).iter().map(|n| upload::reach(n)).collect()))
            .collect();
        // A saved webhook that Discord no longer knows (deleted) shows up here too.
        if let Some(h) = hook
            && let Err(e) = upload::check_webhook(&h)
            && let Some((_, r)) = out.iter_mut().find(|(h, _)| *h == upload::Host::Discord)
        {
            r.push(upload::Reach { host: e, state: "webhook" });
        }
        Ok(out)
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretsSet {
    discord_webhook: bool,
    streamable_password: bool,
}

#[tauri::command]
fn secrets_set(app: AppHandle) -> Result<SecretsSet, String> {
    let f = data_file(&app, "secrets.json")?;
    Ok(SecretsSet { discord_webhook: secrets::is_set(&f, "discord_webhook"), streamable_password: secrets::is_set(&f, "streamable_password") })
}

// ---------- Clip health ----------

/// Health results for these clips (only ones already checked and unchanged).
#[tauri::command]
fn clip_health(app: AppHandle, paths: Vec<String>) -> Result<std::collections::BTreeMap<String, health::Health>, String> {
    Ok(health::results(&data_file(&app, "health.json")?, &paths))
}

#[tauri::command]
fn health_progress() -> health::Progress {
    health::progress()
}

/// Save the longest undamaged part of a damaged clip as "<name> (rescued)", copied untouched.
#[tauri::command]
async fn rescue_clip(app: AppHandle, path: String) -> Result<String, String> {
    let cache = data_file(&app, "health.json")?;
    let out = blocking(move || {
        let h = health::results(&cache, std::slice::from_ref(&path)).remove(&path).ok_or("Check the clip first.")?;
        let (a, b) = h.best.ok_or("There's no undamaged part long enough to keep.")?;
        health::rescue(&path, a, b)
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

/// Try to recover a damaged clip at full length into "<name> (repaired)"; the original stays.
#[tauri::command]
async fn repair_clip(app: AppHandle, path: String) -> Result<String, String> {
    let out = blocking(move || health::repair(&path)).await?;
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

/// Replace a damaged clip with its longest good part (original to the Recycle Bin).
#[tauri::command]
async fn heal_clip(app: AppHandle, path: String) -> Result<String, String> {
    let cache = data_file(&app, "health.json")?;
    let app2 = app.clone();
    let out = blocking(move || {
        let h = health::results(&cache, std::slice::from_ref(&path)).remove(&path).ok_or("Check the clip first.")?;
        let best = h.best.ok_or("There's no undamaged part long enough to keep.")?;
        let healed = health::heal(&path, best)?;
        nas::edited(&settings_of(&app2), std::path::Path::new(&path), std::path::Path::new(&healed));
        // The healed file gets checked again in the background like any new clip.
        health::forget(&cache, &path);
        Ok(healed)
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

/// Medal / OBS / NVIDIA clip folders found on this PC.
#[tauri::command]
async fn recorder_clip_folders() -> Result<Vec<bench::recorders::ClipFolder>, String> {
    blocking(|| Ok(bench::recorders::clip_folders())).await
}

#[tauri::command]
async fn probe_file(path: String) -> Result<library::Probe, String> {
    blocking(move || prepare::probe(&path)).await
}

#[tauri::command]
async fn thumbnail(app: AppHandle, path: String) -> Result<String, String> {
    let dir = app.state::<AppState>().thumbs_dir.clone();
    blocking(move || library::thumbnail(&path, &dir)).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewAudio {
    first_title: String,
    extra: Vec<library::PreviewTrack>,
}

/// Audio tracks Studio has to play next to the video (the video plays track 1 itself).
#[tauri::command]
async fn preview_audio(app: AppHandle, path: String) -> Result<PreviewAudio, String> {
    let cache = app.state::<AppState>().thumbs_dir.with_file_name("preview");
    blocking(move || {
        Ok(PreviewAudio { first_title: library::first_track_title(&path), extra: library::preview_tracks(&path, &cache)? })
    })
    .await
}

#[tauri::command]
async fn remux_mp4(app: AppHandle, path: String) -> Result<String, String> {
    let keep = settings_of(&app).keep_original_after_remux;
    blocking(move || library::remux_mp4(&path, keep)).await
}

/// Run an edit; if `replace`, swap the checked result in for the original.
async fn edit(
    app: AppHandle,
    path: String,
    replace: bool,
    expected_secs: Option<f64>,
    f: impl FnOnce(&str) -> Result<String, String> + Send + 'static,
) -> Result<String, String> {
    let original = path.clone();
    // A network folder has no Recycle Bin. A clip in your NAS folder is replaced by
    // setting the original aside there (kept 30 days); other network folders can't be.
    let nas_dir = settings_of(&app).nas_dir;
    let on_nas = nas::rel_on_nas(&nas_dir, &path).is_some();
    if replace && health::on_network(&path) && !on_nas {
        return Err("This clip is on a network folder, which has no Recycle Bin, so it can't be replaced. Turn off \"Replace the original\" to save your edit as a new file.".into());
    }
    // Edits keep the clip's date: a trim stays where the clip was, not under today.
    let dates = library::dates_of(std::path::Path::new(&original));
    let out = blocking(move || f(&path)).await?;
    if !replace {
        if let Some(d) = dates {
            library::set_dates(std::path::Path::new(&out), d);
        }
        remember_edited(&app, &out);
        return Ok(out);
    }
    let o = original.clone();
    let final_path = if on_nas {
        blocking(move || nas::replace_on_nas(&nas_dir, &o, &out, expected_secs)).await?
    } else {
        blocking(move || library::replace_original(&o, &out, expected_secs)).await?
    };
    if let Some(d) = dates {
        library::set_dates(std::path::Path::new(&final_path), d);
    }
    // Keep the NAS copy in step with the edit.
    nas::edited(&settings_of(&app), std::path::Path::new(&original), std::path::Path::new(&final_path));
    remember_edited(&app, &final_path);
    // Keep an imported file in the list even if its extension changed.
    let mut s = settings_of(&app);
    if let Some(p) = s.external_files.iter_mut().find(|p| **p == original) {
        *p = final_path.clone();
        let _ = set_settings(app, s);
    }
    Ok(final_path)
}

fn duration_of(path: &str) -> Option<f64> {
    library::probe(path).ok().map(|p| p.duration).filter(|d| *d > 0.0)
}

#[tauri::command]
async fn trim_clip(
    app: AppHandle,
    path: String,
    start: f64,
    end: f64,
    exact: bool,
    replace: bool,
    audio: Option<audio_edit::AudioEdit>,
    crop: Option<library::Crop>,
) -> Result<String, String> {
    let audio = audio.unwrap_or_default();
    match crop.filter(|c| !c.is_whole()) {
        Some(c) => edit(app, path, replace, Some(end - start), move |p| library::trim_cropped(p, start, end, &audio, c)).await,
        None => edit(app, path, replace, Some(end - start), move |p| library::trim(p, start, end, exact, &audio)).await,
    }
}

/// Loudness over time for each audio track, for Studio's audio lanes.
#[tauri::command]
async fn clip_waveforms(app: AppHandle, path: String) -> Result<Vec<audio_edit::Waveform>, String> {
    let cache = app.state::<AppState>().thumbs_dir.with_file_name("preview");
    blocking(move || {
        let n = prepare::probe(&path)?.audio_tracks.min(8);
        audio_edit::waveforms(&path, n, &cache)
    })
    .await
}

#[tauri::command]
async fn export_h264(app: AppHandle, path: String, replace: bool, audio: Option<audio_edit::AudioEdit>, crop: Option<library::Crop>) -> Result<String, String> {
    let secs = duration_of(&path);
    let audio = audio.unwrap_or_default();
    edit(app, path, replace, secs, move |p| library::export_h264(p, &audio, crop)).await
}

/// Clips changed in the app (trimmed, healed, exported in place), for the
/// montage's "edited clips" choice. Kept in app data as a list of paths.
pub(crate) fn remember_edited(app: &AppHandle, path: &str) {
    if let Ok(file) = data_file(app, "edited.json") {
        let mut list: Vec<String> = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if !list.iter().any(|p| p == path) {
            list.push(path.to_string());
            let _ = std::fs::write(&file, serde_json::to_string(&list).unwrap_or_default());
        }
    }
}

/// Windows' accent colour as "#rrggbb", made light enough to read on a dark
/// background (the way Windows' own dark mode shows it).
#[tauri::command]
fn system_accent() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::HSTRING;
    let (mut v, mut len) = (0u32, 4u32);
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(r"Software\Microsoft\Windows\DWM"),
            &HSTRING::from("AccentColor"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut len),
        )
        .ok()
        .ok()?;
    }
    // Stored as 0xAABBGGRR.
    let (r, g, b) = ((v & 0xff) as f32, ((v >> 8) & 0xff) as f32, ((v >> 16) & 0xff) as f32);
    Some(lighten_for_dark(r, g, b))
}

/// Mix with white until it's bright enough on a dark background.
fn lighten_for_dark(r: f32, g: f32, b: f32) -> String {
    let lum = |r: f32, g: f32, b: f32| 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let mut t = 0.0;
    let mix = |c: f32, t: f32| c + (255.0 - c) * t;
    while lum(mix(r, t), mix(g, t), mix(b, t)) < 150.0 && t < 0.9 {
        t += 0.05;
    }
    format!("#{:02x}{:02x}{:02x}", mix(r, t).round() as u8, mix(g, t).round() as u8, mix(b, t).round() as u8)
}

#[tauri::command]
fn edited_clips(app: AppHandle) -> Vec<String> {
    data_file(&app, "edited.json")
        .ok()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Render a montage into <clips>/Montages; "montage-progress" events carry 0..1.
#[tauri::command]
async fn render_montage(app: AppHandle, montage: montage::Montage) -> Result<String, String> {
    let s = settings_of(&app);
    let out = montage::out_path(std::path::Path::new(&s.clips_dir));
    let a = app.clone();
    let path = blocking(move || {
        let mut last = -1.0;
        montage::render(&montage, &out, |p| {
            if p - last >= 0.005 || p >= 1.0 {
                last = p;
                let _ = a.emit("montage-progress", p);
            }
        })
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    Ok(path)
}

#[tauri::command]
fn cancel_montage() {
    montage::cancel();
}

/// Make a diagnostics report (optionally about one clip), save it in
/// Downloads and return its path. See diag.rs for what's in it.
#[tauri::command]
async fn run_diagnostics(app: AppHandle, clip: Option<String>) -> Result<String, String> {
    let s = settings_of(&app);
    let status = app.state::<AppState>().engine.status();
    let version = app.package_info().version.to_string();
    blocking(move || {
        let nas = nas::state();
        let extra = vec![
            ("Recorder", serde_json::to_string_pretty(&status).unwrap_or_default()),
            ("Settings", diag::settings_json(&s)),
            (
                "Storage",
                format!(
                    "Clips folder: {} ({})\nPrepared clip details: {}",
                    s.clips_dir,
                    diag::free_space_line(&s.clips_dir),
                    if s.prepare_thumbnails { "on" } else { "off" }
                ),
            ),
            (
                "NAS",
                if s.nas_dir.is_empty() {
                    "off".to_string()
                } else {
                    // The NAS's own name stays private: only how it's doing.
                    format!(
                        "on, reachable: {}, paused: {}, pending: {} ({} bytes), copied this run: {}, last error: {}",
                        nas.reachable,
                        nas.paused,
                        nas.pending,
                        nas.pending_bytes,
                        nas.copied,
                        nas.last_error.unwrap_or_default().replace(&s.nas_dir, "<NAS>")
                    )
                },
            ),
            ("Health check", serde_json::to_string(&health::progress()).unwrap_or_default()),
        ];
        let mut text = diag::report(&version, &extra, clip.as_deref());
        if !s.nas_dir.is_empty() {
            // The settings JSON spells it with doubled backslashes.
            text = text.replace(&s.nas_dir, "<NAS>").replace(&s.nas_dir.replace('\\', "\\\\"), "<NAS>");
        }
        diag::save(&text).map(|p| p.to_string_lossy().into_owned())
    })
    .await
}

// ---------- Finding the game from the picture (see game_detect.rs) ----------

#[tauri::command]
fn detect_status() -> game_detect::Status {
    game_detect::status()
}

/// Download the image model and runtime once (about 106 MB); "detect-progress"
/// carries the stage and bytes (downloading, checking, installing, ready).
#[tauri::command]
async fn detect_install(app: AppHandle) -> Result<(), String> {
    let a = app.clone();
    let out = blocking(move || {
        let mut last: (&str, u64) = ("", 0);
        game_detect::install(|step| {
            // At most every ~1 MB while downloading, and every change of stage.
            if step.stage != last.0 || step.done >= last.1 + (1 << 20) || step.done == step.total {
                last = (step.stage, step.done);
                let _ = a.emit("detect-progress", step);
            }
        })
    })
    .await;
    let _ = app.emit("detect-changed", ());
    out
}

#[tauri::command]
async fn game_suggestions(app: AppHandle) -> Result<Vec<game_detect::Suggestion>, String> {
    let s = settings_of(&app);
    blocking(move || {
        let linked = linked_roots(&s);
        let all = library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &[], &linked, &s.nas_dir);
        Ok(game_detect::suggestions(&all, &s.detect_dismissed))
    })
    .await
}

/// Move the chosen clips into their game's folder (logged, so Undo works like other moves).
#[tauri::command]
async fn apply_game_sort(app: AppHandle, picks: Vec<(String, String)>) -> Result<consolidate::Outcome, String> {
    let s = settings_of(&app);
    let dir = moves_dir(&app)?;
    let out = blocking(move || {
        let plan = game_detect::plan(std::path::Path::new(&s.clips_dir), &picks);
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join(format!("{}-Sorted_by_game.jsonl", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        Ok(consolidate::run(&plan, &log, |_, _| {}))
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    nas::wake();
    Ok(out)
}

/// Don't suggest a game for these clips again.
#[tauri::command]
fn dismiss_game_suggestions(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    let mut s = settings_of(&app);
    for p in paths {
        if !s.detect_dismissed.contains(&p) {
            s.detect_dismissed.push(p);
        }
    }
    set_settings(app, s)
}

/// Sets of library paths that hold the very same clip (checked byte for byte).
#[tauri::command]
async fn find_duplicates(app: AppHandle) -> Result<Vec<Vec<String>>, String> {
    let s = settings_of(&app);
    blocking(move || {
        let linked = linked_roots(&s);
        // Only files on this PC. A clip and its NAS copy are the same clip on purpose,
        // and nothing on the NAS is ever offered for removal as a "duplicate".
        let all = library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &[], &linked, &s.nas_dir);
        let list: Vec<(String, u64)> = all
            .into_iter()
            .filter(|c| c.location != "nas" && !c.path.starts_with(r"\\") && !health::on_network(&c.path))
            .map(|c| (c.path, c.size))
            .collect();
        Ok(duplicates::find(&list))
    })
    .await
}

/// Where the app's player loads clips from (a local server that streams only
/// the bytes it asks for), or None to use the file protocol.
#[tauri::command]
fn media_base() -> Option<String> {
    media_server::base()
}

/// Use a picture as a game's logo, or `None` to go back to the automatic one.
#[tauri::command]
async fn set_game_logo(app: AppHandle, name: String, picture: Option<String>) -> Result<(), String> {
    blocking(move || game_icons::set_logo(&name, picture.as_deref().map(std::path::Path::new))).await?;
    let _ = app.emit("icons-changed", ());
    Ok(())
}

/// A game's logo (a picture you chose, Steam's, or its program's icon) as a file path, or None for a letter tile.
#[tauri::command]
async fn game_icon(app: AppHandle, name: String) -> Result<Option<String>, String> {
    let online = settings_of(&app).online_game_logos;
    blocking(move || Ok(game_icons::icon(&name, online).map(|p| p.to_string_lossy().into_owned()))).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GpuEncoders {
    gpu: String,
    /// "av1" | "hevc" | "h264" this GPU can encode
    supported: Vec<&'static str>,
    /// What "auto" records with
    auto: &'static str,
}

/// Which encoders this PC's NVIDIA GPU has (for the Encoder setting).
#[tauri::command]
async fn gpu_encoders() -> Result<GpuEncoders, String> {
    use crate::native::nvenc;
    blocking(|| {
        Ok(GpuEncoders {
            gpu: nvenc::nvidia_gpu_name(),
            supported: nvenc::supported_codecs().into_iter().map(|c| c.setting()).collect(),
            auto: nvenc::effective_encoder("auto"),
        })
    })
    .await
}

#[tauri::command]
async fn export_audio(
    path: String,
    start: f64,
    end: f64,
    track: Option<usize>,
    label: String,
    audio: Option<audio_edit::AudioEdit>,
) -> Result<String, String> {
    let audio = audio.unwrap_or_default();
    blocking(move || library::export_audio(&path, start, end, track, &label, &audio)).await
}

#[tauri::command]
async fn compress_clip(
    app: AppHandle,
    path: String,
    target_mb: u32,
    replace: bool,
    audio: Option<audio_edit::AudioEdit>,
    crop: Option<library::Crop>,
) -> Result<String, String> {
    let secs = duration_of(&path);
    let audio = audio.unwrap_or_default();
    edit(app, path, replace, secs, move |p| library::compress(p, target_mb, &audio, crop)).await
}

/// Open Explorer with the file selected (or the folder itself, for a folder).
#[tauri::command]
fn reveal_file(path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err("That file isn't there anymore.".into());
    }
    let mut cmd = std::process::Command::new("explorer");
    // Explorer wants exactly: /select,"C:\path with spaces\file.mkv"
    // (Rust's normal quoting wraps the whole thing, which Explorer ignores.)
    if p.is_dir() {
        cmd.raw_arg(format!("\"{path}\""));
    } else {
        cmd.raw_arg(format!("/select,\"{path}\""));
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Move clips to the Recycle Bin. Imported files stay listed so Undo brings them back.
#[tauri::command]
async fn delete_clips(app: AppHandle, paths: Vec<String>) -> Result<library::DeleteResult, String> {
    let s = settings_of(&app);
    let res = blocking(move || {
        Ok(library::delete_clips(&paths, PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &s.linked_folders, &s.nas_dir))
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    Ok(res)
}

#[tauri::command]
async fn restore_clips(app: AppHandle, paths: Vec<String>) -> Result<usize, String> {
    let n = blocking(move || library::restore_clips(&paths)).await?;
    let _ = app.emit("clips-changed", ());
    Ok(n)
}

#[tauri::command]
fn add_external_files(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    let mut s = settings_of(&app);
    for p in paths {
        if !s.external_files.contains(&p) {
            s.external_files.push(p);
        }
    }
    set_settings(app, s)
}

#[tauri::command]
fn remove_external_file(app: AppHandle, path: String) -> Result<(), String> {
    let mut s = settings_of(&app);
    s.external_files.retain(|p| p != &path);
    set_settings(app, s)
}

#[tauri::command]
fn open_in_studio(app: AppHandle, path: String) {
    show_main(&app);
    let _ = app.emit_to("main", "open-in-studio", path);
    if let Some(f) = app.get_webview_window("flyout") {
        let _ = f.hide();
    }
}

#[tauri::command]
fn show_main_window(app: AppHandle) {
    show_main(&app);
}

#[tauri::command]
fn get_update_state(app: AppHandle) -> updater::UpdateState {
    updater::state(&app)
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<Option<String>, String> {
    let auto = settings_of(&app).auto_update;
    updater::check(&app, auto).await
}

/// Download (if needed) and install now. Hindsight restarts on the new version.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    if !updater::has_ready_update(&app) {
        updater::check(&app, true).await?;
    }
    updater::install(&app)
}

#[tauri::command]
fn start_bench(app: AppHandle, options: bench::run::Options) -> Result<(), String> {
    bench::run::start(app, options)
}

#[tauri::command]
fn cancel_bench() {
    bench::run::cancel();
}

#[tauri::command]
fn bench_state() -> (bool, Option<bench::run::Report>) {
    (bench::run::is_running(), bench::run::last_report())
}

#[tauri::command]
fn hide_flyout(app: AppHandle) {
    if let Some(f) = app.get_webview_window("flyout") {
        let _ = f.hide();
    }
}

#[cfg(test)]
mod tests {
    /// Renders the tray icon with its live dot so it can be looked at.
    /// Input/output: raw 64x64 RGBA in %TEMP%.
    #[test]
    #[ignore]
    fn tray_dot_preview() {
        let dir = std::env::temp_dir();
        let px = std::fs::read(dir.join("rewind_icon.rgba")).unwrap();
        let base = tauri::image::Image::new_owned(px, 64, 64);
        let live = super::tray_icon(&base, true);
        std::fs::write(dir.join("rewind_icon_live.rgba"), live.rgba()).unwrap();
        let idle = super::tray_icon(&base, false);
        assert_eq!(idle.rgba(), base.rgba(), "no dot when idle");
    }
}

// ---------- app ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // `Rewind.exe --save-clip` etc. control the running copy; a plain launch shows it.
            if !control::handle_args(app, &args) {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let s = settings_of(app);
                    if Shortcut::from_str(&s.record_hotkey).ok().as_ref() == Some(shortcut) {
                        in_background(app, |a| toggle_record_now(a).map(|_| ()));
                    } else {
                        in_background(app, |a| save_now(a, None).map(|_| ()));
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            ffmpeg::init(app.path().resource_dir().ok());

            let cfg_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&cfg_dir)?;
            let settings_path = cfg_dir.join("settings.json");
            let mut s = settings::load(&settings_path);
            audio::set_mic_gain_db(s.mic_gain_db);
            if s.clips_dir.is_empty() {
                let videos = app.path().video_dir().unwrap_or_else(|_| cfg_dir.clone());
                s.clips_dir = videos.join("Hindsight").to_string_lossy().into_owned();
            }
            let _ = std::fs::create_dir_all(&s.clips_dir);
            let _ = settings::save(&settings_path, &s);

            let local = app.path().app_local_data_dir()?;
            // Preview audio is cheap to rebuild; start each session clean.
            let _ = std::fs::remove_dir_all(local.join("preview"));
            let scratch_dir = local.join("scratch");
            let _ = std::fs::create_dir_all(&scratch_dir);
            let shared = Arc::new(Mutex::new(s.clone()));
            let engine = engine::Engine::start(shared.clone(), local.join("buffer"));
            app.manage(AppState {
                settings: shared,
                settings_path,
                engine,
                thumbs_dir: local.join("thumbs"),
                flyout_hidden_at: Mutex::new(Instant::now() - Duration::from_secs(5)),
            });

            // A recording interrupted by a crash or power cut is rebuilt from its pieces.
            {
                let (h, clips, scratch) = (handle.clone(), PathBuf::from(&s.clips_dir), scratch_dir);
                thread::spawn(move || {
                    for f in native::pipeline::recover_saves(&clips).into_iter().chain(engine::recover_recordings(&clips, &scratch)) {
                        let _ = h.emit("clip-saved", &f);
                    }
                });
            }

            app.manage(updater::Updates::default());
            updater::start(handle.clone());
            nas::start(handle.clone());
            // Remember the game you're playing, so a clip saved while tabbed out still goes to it.
            thread::spawn(|| loop {
                games::observe();
                thread::sleep(Duration::from_secs(2));
            });
            game_icons::init(handle.state::<AppState>().thumbs_dir.with_file_name("icons"));
            // Finding the game from the picture: only with the setting on and the model downloaded.
            if let Ok(ai) = handle.path().app_data_dir().map(|d| d.join("ai")) {
                game_detect::init(ai);
                let (d1, d2, d3, d4) = (handle.clone(), handle.clone(), handle.clone(), handle.clone());
                game_detect::start(
                    move || {
                        let s = settings_of(&d1);
                        let linked = linked_roots(&s);
                        library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &[], &linked, &s.nas_dir)
                    },
                    move || settings_of(&d2).game_detect,
                    move || {
                        let st = d3.state::<AppState>();
                        games::game_running() || st.engine.is_recording() || engine::saves_in_progress() > 0 || bench::run::is_running()
                    },
                    move || {
                        let _ = d4.emit("detect-changed", ());
                    },
                );
            }
            // Clip details and thumbnails, ready before you scroll to them.
            if let Ok(file) = data_file(&handle, "probes.json") {
                prepare::init(file);
                {
                    let s = settings_of(&handle);
                    library::set_twin(&s.nas_dir, &s.clips_dir);
                }
                let thumbs = handle.state::<AppState>().thumbs_dir.clone();
                let preview = thumbs.with_file_name("preview");
                let (p1, p2, p3) = (handle.clone(), handle.clone(), handle.clone());
                prepare::start(
                    thumbs,
                    move || {
                        let s = settings_of(&p1);
                        let linked = linked_roots(&s);
                        library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &nas::listing(&s.nas_dir), &linked, &s.nas_dir)
                            .into_iter()
                            .map(|c| c.path)
                            .collect()
                    },
                    move || settings_of(&p2).prepare_thumbnails,
                    move || {
                        let st = p3.state::<AppState>();
                        games::game_running() || st.engine.is_recording() || engine::saves_in_progress() > 0 || bench::run::is_running()
                    },
                    move |p: &str| {
                        if let Ok(pr) = prepare::probe(p) {
                            let _ = audio_edit::waveforms(p, pr.audio_tracks.min(8), &preview);
                        }
                        let _ = library::preview_tracks(p, &preview);
                    },
                );
            }
            // Clip health check: one clip at a time, only while the PC is free.
            if let Ok(cache) = data_file(&handle, "health.json") {
                let (h1, h2, h3, h4) = (handle.clone(), handle.clone(), handle.clone(), handle.clone());
                health::start(
                    cache,
                    move || {
                        let s = settings_of(&h1);
                        let linked = linked_roots(&s);
                        library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &[], &linked, &s.nas_dir)
                            .into_iter()
                            .map(|c| c.path)
                            .collect()
                    },
                    move || settings_of(&h2).health_check,
                    move || {
                        let st = h3.state::<AppState>();
                        games::foreground_game().is_some() || st.engine.is_recording() || engine::saves_in_progress() > 0 || bench::run::is_running()
                    },
                    move || {
                        let _ = h4.emit("clips-changed", ());
                    },
                );
            }
            control::start(handle.clone());
            media_server::start();

            if let Err(e) = apply_hotkeys(&handle, &s.hotkey, &s.record_hotkey) {
                crate::log!("{e}");
            }
            apply_autostart(&handle, s.launch_on_startup);

            // Tray icon: left click opens the flyout, right click shows the menu.
            let save = MenuItem::with_id(app, "save", "Save clip", true, None::<&str>)?;
            let record = MenuItem::with_id(app, "record", "Start / stop recording", true, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open Hindsight", true, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Pause / resume buffer", true, None::<&str>)?;
            let updates = MenuItem::with_id(app, "updates", "Check for updates", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Hindsight", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&save, &record, &open, &toggle, &sep, &updates, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .tooltip("Hindsight")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "save" => in_background(app, |a| save_now(a, None).map(|_| ())),
                    "record" => in_background(app, |a| toggle_record_now(a).map(|_| ())),
                    "open" => show_main(app),
                    "toggle" => {
                        let mut s = settings_of(app);
                        s.buffer_enabled = !s.buffer_enabled;
                        let _ = set_settings(app.clone(), s);
                    }
                    "updates" => {
                        // Show Settings → Updates and check there; the result appears on that page.
                        show_main(app);
                        let _ = app.emit("show-view", "settings");
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let auto = settings_of(&app).auto_update;
                            let _ = updater::check(&app, auto).await;
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = e
                    {
                        toggle_flyout(tray.app_handle(), position);
                    }
                })
                .build(app)?;

            // Push engine status to both windows once a second, announce
            // recordings the engine finished on its own (e.g. low disk space),
            // and keep the tray icon's red dot in step with capture.
            let h = handle.clone();
            let base_icon = app.default_window_icon().cloned().expect("app icon").to_owned();
            thread::spawn(move || {
                let mut shown: Option<(bool, bool)> = None;
                loop {
                thread::sleep(Duration::from_secs(1));
                let engine = &h.state::<AppState>().engine;
                let st = engine.status();
                let state = (st.running, st.recording);
                if shown != Some(state) {
                    shown = Some(state);
                    if let Some(tray) = h.tray_by_id("main") {
                        let _ = tray.set_icon(Some(tray_icon(&base_icon, st.running)));
                        let tip = match state {
                            (_, true) => "Hindsight · Recording",
                            (true, false) => "Hindsight · Replay buffer on",
                            _ => "Hindsight · Paused",
                        };
                        let _ = tray.set_tooltip(Some(tip));
                    }
                }
                let (finished, err) = engine.take_finished_recordings();
                for f in finished {
                    let _ = h.emit("clip-saved", &f);
                }
                if let Some(e) = err {
                    let _ = h.emit("clip-error", e);
                    let _ = h.emit("recording-changed", false);
                }
                let _ = h.emit("engine-status", st);
                }
            });

            // Launched by a command (e.g. Stream Deck) while not running: start
            // quietly in the tray. There's nothing buffered yet to save.
            let by_command = std::env::args().any(|a| a.starts_with("--") && a != "--minimized");
            let minimized = by_command || (std::env::args().any(|a| a == "--minimized") && s.start_minimized);
            if !minimized {
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| match (window.label(), event) {
            ("main", WindowEvent::CloseRequested { api, .. }) => {
                // Closing the window keeps Hindsight running in the tray.
                api.prevent_close();
                let _ = window.emit("main-hidden", ());
                let _ = window.hide();
            }
            ("main", WindowEvent::Resized(_)) => {
                if window.is_minimized().unwrap_or(false) {
                    let _ = window.emit("main-hidden", ());
                }
            }
            ("flyout", WindowEvent::Focused(false)) => {
                let _ = window.hide();
                *window.app_handle().state::<AppState>().flyout_hidden_at.lock().unwrap() = Instant::now();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            screen_preview,
            nas_check,
            nas_confirm,
            nas_pair,
            get_status,
            restart_capture,
            list_monitors,
            list_audio_inputs,
            save_clip,
            toggle_recording,
            list_clips,
            probe_file,
            thumbnail,
            preview_audio,
            remux_mp4,
            trim_clip,
            clip_waveforms,
            export_h264,
            export_audio,
            gpu_encoders,
            game_icon,
            set_game_logo,
            media_base,
            old_folders,
            remove_old_folder,
            find_duplicates,
            detect_status,
            detect_install,
            game_suggestions,
            apply_game_sort,
            dismiss_game_suggestions,
            run_diagnostics,
            free_local_copy,
            render_montage,
            cancel_montage,
            edited_clips,
            system_accent,
            compress_clip,
            reveal_file,
            delete_clips,
            restore_clips,
            add_external_files,
            remove_external_file,
            open_in_studio,
            show_main_window,
            hide_flyout,
            get_update_state,
            check_for_updates,
            install_update,
            get_nas_state,
            sync_nas_now,
            stop_nas_sync,
            recorder_clip_folders,
            upload_clip,
            upload_history,
            copy_text,
            set_secret,
            secrets_set,
            check_sharing,
            clip_health,
            health_progress,
            rescue_clip,
            heal_clip,
            repair_clip,
            plan_move_into_rewind,
            move_into_rewind,
            last_move,
            undo_last_move,
            similar_game_folders,
            find_clip_folders,
            shrink_clip,
            plan_import,
            import_folder,
            merge_game_folders,
            keep_folders_separate,
            start_bench,
            cancel_bench,
            bench_state,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Hindsight")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // A downloaded update installs on quit (after the engine stops).
                if settings_of(app).auto_update && updater::has_ready_update(app) {
                    let _ = updater::install(app);
                }
                // Finishes an active recording before quitting.
                app.state::<AppState>().engine.shutdown();
            }
        });
}
