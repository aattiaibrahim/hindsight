mod app_audio;
mod encoders;
mod audio_sessions;
mod scrub;
mod session_end;
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
mod nas_connect;
mod old_folders;
mod native;
mod secrets;
mod settings;
mod sound;
mod updater;
mod upload;

use settings::Settings;
use std::path::{Path, PathBuf};
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

/// "Always one monitor": keep the chosen screen's hardware id next to its
/// DISPLAY number, so it's found again when Windows renumbers the displays
/// (after a driver update, a cable swap, waking up…). A screen picked by
/// number alone (an older version, the Stream Deck) gets its id here, and
/// one that was renumbered gets its new number.
fn pin_monitor(s: &mut Settings, old: Option<&Settings>) {
    if s.monitor_mode != "fixed" {
        return;
    }
    let mons = monitors::list();
    // "Always one monitor" with none chosen yet (picked in the tray, say): the
    // one being recorded now, so it's set and stays put.
    if s.fixed_monitor.is_empty() {
        if let Some(m) = mons.iter().find(|m| m.primary).or(mons.first()) {
            s.fixed_monitor = m.name.clone();
            s.fixed_monitor_id = m.id.clone();
        }
        return;
    }
    // Picked by number only (the id wasn't sent with it): look the id up again.
    if old.is_some_and(|o| o.fixed_monitor != s.fixed_monitor && o.fixed_monitor_id == s.fixed_monitor_id) {
        s.fixed_monitor_id.clear();
    }
    if let Some(m) = monitors::find_fixed(&s.fixed_monitor, &s.fixed_monitor_id, &mons) {
        if m.name != s.fixed_monitor {
            crate::log!("monitor: the chosen screen is now {} (was {})", m.name, s.fixed_monitor);
        }
        s.fixed_monitor = m.name.clone();
        s.fixed_monitor_id = m.id.clone();
    }
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let mut settings = settings.sanitized();
    apply_log(&app, &settings);
    audio::set_mic_gain_db(settings.mic_gain_db);
    let st = app.state::<AppState>();
    let old = st.settings.lock().unwrap().clone();
    pin_monitor(&mut settings, Some(&old));
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
    library::set_nas_app(&settings.nas_dir, &settings.nas_server);
    if old.launch_on_startup != settings.launch_on_startup {
        apply_autostart(&app, settings.launch_on_startup);
    }
    if old.nas_dir != settings.nas_dir || old.nas_remove_local != settings.nas_remove_local || old.clips_dir != settings.clips_dir {
        nas::wake();
    }
    if old.linked_folders != settings.linked_folders || old.nas_dir != settings.nas_dir {
        let _ = app.emit("clips-changed", ());
    }
    // The game finder's suggestions depend on these: show the new ones.
    if old.not_game_folders != settings.not_game_folders || old.detect_dismissed != settings.detect_dismissed || old.detect_folders != settings.detect_folders {
        let _ = app.emit("detect-changed", ());
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

/// Sign in to the NAS (Settings → Library & NAS → Connect) and list its shared folders.
#[tauri::command]
async fn nas_sign_in(host: String, user: String, password: String, remember: bool) -> Result<Vec<nas_connect::NasShare>, String> {
    tauri::async_runtime::spawn_blocking(move || nas_connect::connect(&host, &user, &password, remember))
        .await
        .map_err(|e| e.to_string())?
}

/// Open the share picked after signing in (making the folder in it if asked) and return its path.
#[tauri::command]
async fn nas_open_share(path: String, user: String, password: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || nas_connect::open_share(&path, &user, &password))
        .await
        .map_err(|e| e.to_string())?
}

/// The user name saved in Credential Manager for this NAS (never the password).
#[tauri::command]
fn nas_saved_login(host: String) -> Option<nas_connect::SavedLogin> {
    nas_connect::saved_login(&host)
}

#[tauri::command]
fn nas_forget_login(host: String) {
    nas_connect::forget(&host)
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
        // A game in front, or left within the last 10 minutes and still open. (Not
        // game_running: that counts a game left open in the background for hours,
        // which kept the preview paused with no game being played.)
        if let Some(g) = games::current_game() {
            return Err(format!("game:{g}"));
        }
        let mons = monitors::list();
        let m = monitors::pick(&s.monitor_mode, &s.fixed_monitor, &s.fixed_monitor_id, &mons).ok_or("No screen found")?;
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

// ---------- A move into Hindsight, in the background ----------

/// What the status bar shows about a move running in the background.
#[derive(serde::Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct MoveStatus {
    running: bool,
    /// "Moving" or "Copying"
    verb: String,
    /// The folders it comes from ("Recordings, Clips")
    label: String,
    done: usize,
    total: usize,
    /// Waiting: a game is running, or a clip is being saved or recorded
    paused: bool,
    /// The game it's waiting for (None: a save or a recording)
    paused_for: Option<String>,
    /// "Move anyway" was pressed: it no longer waits for games
    forced: bool,
    stopping: bool,
    /// When it's over: what happened ("Moved 810 clips", "Stopped: 120 moved…")
    finished: Option<String>,
}
static MOVE_STATUS: Mutex<Option<MoveStatus>> = Mutex::new(None);
static MOVE_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// "Move anyway": don't wait for a game (a game left open in the background kept a
/// move waiting with nothing being played).
static MOVE_FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Carry on with the background move while a game is open (it still waits for saves).
#[tauri::command]
fn move_anyway(app: AppHandle) {
    MOVE_FORCE.store(true, std::sync::atomic::Ordering::SeqCst);
    move_status_set(&app, |s| s.forced = true);
}

fn move_status_set(app: &AppHandle, f: impl FnOnce(&mut MoveStatus)) {
    let snap = {
        let mut s = MOVE_STATUS.lock().unwrap();
        let st = s.get_or_insert_with(MoveStatus::default);
        f(st);
        st.clone()
    };
    let _ = app.emit("move-status", snap);
}

/// Is a move into Hindsight running in the background (the updater waits for it)?
pub(crate) fn move_running() -> bool {
    MOVE_STATUS.lock().unwrap().as_ref().is_some_and(|s| s.running)
}

#[tauri::command]
fn move_status() -> Option<MoveStatus> {
    MOVE_STATUS.lock().unwrap().clone()
}

/// Stop the background move after the file it's on.
#[tauri::command]
fn stop_move(app: AppHandle) {
    MOVE_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    move_status_set(&app, |s| s.stopping = true);
}

/// Run a move plan in the background: progress for the status bar, waiting while
/// a game runs or a clip is saved or recorded (copying hundreds of GB shouldn't
/// cost FPS or slow a save), and Stop.
fn run_in_background(app: &AppHandle, plan: &consolidate::Plan, log: &Path, verb: &str) -> consolidate::Outcome {
    if MOVE_STATUS.lock().unwrap().as_ref().is_some_and(|s| s.running) {
        let mut out = consolidate::Outcome::default();
        out.failed.push((plan.label.clone(), "another move is still running; try again when it's done".into()));
        return out;
    }
    MOVE_STOP.store(false, std::sync::atomic::Ordering::SeqCst);
    MOVE_FORCE.store(false, std::sync::atomic::Ordering::SeqCst);
    let total = plan.moves.len();
    move_status_set(app, |s| {
        *s = MoveStatus { running: true, verb: verb.into(), label: plan.label.clone(), total, ..Default::default() };
    });
    let (a, h) = (app.clone(), app.clone());
    let out = consolidate::run_held(
        plan,
        log,
        |done, total| move_status_set(&a, |s| {
            s.done = done;
            s.total = total;
        }),
        || {
            if MOVE_STOP.load(std::sync::atomic::Ordering::SeqCst) {
                return consolidate::Hold::Stop;
            }
            let st = h.state::<AppState>();
            let game = if MOVE_FORCE.load(std::sync::atomic::Ordering::SeqCst) { None } else { games::running_game() };
            let busy = game.is_some() || st.engine.is_recording() || engine::saves_in_progress() > 0;
            let (was, was_for) = MOVE_STATUS.lock().unwrap().as_ref().map(|s| (s.paused, s.paused_for.clone())).unwrap_or_default();
            if busy != was || game != was_for {
                move_status_set(&h, |s| {
                    s.paused = busy;
                    s.paused_for = game.clone();
                });
            }
            if busy { consolidate::Hold::Wait } else { consolidate::Hold::Go }
        },
    );
    let past = if verb == "Copying" { "Copied" } else { "Moved" };
    let summary = if out.stopped > 0 {
        format!("Stopped: {} {} clips, {} left where they were", past.to_lowercase(), out.moved, out.stopped)
    } else if out.failed.is_empty() {
        format!("{past} {} clips into Hindsight", out.moved)
    } else {
        format!("{past} {} clips; {} couldn't be", out.moved, out.failed.len())
    };
    move_status_set(app, |s| {
        s.running = false;
        s.paused = false;
        s.stopping = false;
        s.done = s.total;
        s.finished = Some(summary);
    });
    out
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
        Ok(run_in_background(&a, &plan, &log, if plan.copy { "Copying" } else { "Moving" }))
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
    let nas_dir = settings_of(&app).nas_dir;
    let out = blocking(move || {
        let out = consolidate::undo(&log);
        nas::refresh_listing(&nas_dir);
        Ok(out)
    })
    .await?;
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
        Ok(run_in_background(&a, &plan, &log, if copy { "Copying" } else { "Moving" }))
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    nas::wake();
    Ok(out)
}

// ---------- Game folders under different spellings ----------

/// "Find the game" on one of your own folders: the game finder looks through its clips
/// too, and its suggestions come up in Notifications like the rest.
#[tauri::command]
fn find_game_in_folder(app: AppHandle, folder: String) -> Result<(), String> {
    change_settings(&app, |s| {
        if !s.detect_folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
            s.detect_folders.push(folder.clone());
        }
        s.game_detect = true;
    })?;
    let _ = app.emit("detect-changed", ());
    Ok(())
}

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

/// Settings → Detailed log: everything `log!` prints also goes to logs\hindsight.log.
fn apply_log(app: &AppHandle, s: &Settings) {
    let dir = data_file(app, "logs").ok().filter(|_| s.detailed_log);
    diag::set_log_file(dir.as_deref());
}

/// A message from the interface (an error it showed, a crash) for the detailed log.
#[tauri::command]
fn log_ui(text: String) {
    let text: String = text.chars().take(2000).collect();
    crate::log!("[ui] {text}");
}

/// Open the folder with the detailed log in Explorer.
#[tauri::command]
fn open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = data_file(&app, "logs")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer").arg(&dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
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
        let started = std::time::Instant::now();
        let file = upload::prepare(path, host, &work)?;
        crate::log!(
            "[upload] {} to {}: {} MB → {} MB ({}) in {:.1}s",
            path,
            host.label(),
            fs_len(std::path::Path::new(path)) / 1_000_000,
            fs_len(&file) / 1_000_000,
            if file == std::path::Path::new(path) { "sent as it is" } else { "made an MP4" },
            started.elapsed().as_secs_f64()
        );
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
        Ok(u) => {
            crate::log!("[upload] done: {} ({} MB)", u.url, u.bytes / 1_000_000);
            emit("done", u.bytes, u.bytes, Some(u.url.clone()), None, upload::link_warning(&u.url))
        }
        Err(e) => {
            crate::log!("[upload] {path} failed: {e}");
            emit("error", 0, 0, None, Some(e.clone()), None)
        }
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
/// Off the main thread: it reads every checked clip's size and date (over the network
/// for NAS clips), and on the main thread that held up every other request, so the
/// library's cards waited for their thumbnails and details.
#[tauri::command]
async fn clip_health(app: AppHandle, paths: Vec<String>) -> Result<std::collections::BTreeMap<String, health::Health>, String> {
    let file = data_file(&app, "health.json")?;
    blocking(move || Ok(health::results(&file, &paths))).await
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
    let report = progress_to(&app, &path, duration_of(&path));
    let out = blocking(move || ffmpeg::with_progress(report, || health::repair(&path))).await?;
    let _ = app.emit("clips-changed", ());
    Ok(out)
}

/// Replace a damaged clip with its longest good part (original to the Recycle Bin).
#[tauri::command]
async fn heal_clip(app: AppHandle, path: String) -> Result<String, String> {
    let cache = data_file(&app, "health.json")?;
    let app2 = app.clone();
    let report = progress_to(&app, &path, duration_of(&path));
    let out = blocking(move || {
        let h = health::results(&cache, std::slice::from_ref(&path)).remove(&path).ok_or("Check the clip first.")?;
        let best = h.best.ok_or("There's no undamaged part long enough to keep.")?;
        let healed = ffmpeg::with_progress(report, || health::heal(&path, best))?;
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

/// Lengths already known for these clips ((path, size, modified) each), 0 where not:
/// one quick call, nothing read from the files.
#[tauri::command]
fn known_lengths(clips: Vec<(String, u64, i64)>) -> Vec<f64> {
    prepare::known_lengths(&clips)
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
    /// A track that's the others already mixed (Medal's "All Audio"): heard twice if played with them.
    mix: Option<usize>,
}

/// Audio tracks Studio has to play next to the video (the video plays track 1 itself).
#[tauri::command]
async fn preview_audio(app: AppHandle, path: String) -> Result<PreviewAudio, String> {
    let cache = app.state::<AppState>().thumbs_dir.with_file_name("preview");
    blocking(move || {
        let mix = library::mix_copy(&library::track_titles(&path));
        Ok(PreviewAudio { first_title: library::first_track_title(&path), extra: library::preview_tracks(&path, &cache)?, mix })
    })
    .await
}

#[tauri::command]
async fn remux_mp4(app: AppHandle, path: String) -> Result<String, String> {
    let keep = settings_of(&app).keep_original_after_remux;
    let report = progress_to(&app, &path, duration_of(&path));
    blocking(move || ffmpeg::with_progress(report, || library::remux_mp4(&path, keep))).await
}

/// Report an ffmpeg job's progress on `path` to the page, as "edit-progress"
/// { path, pct } (Studio, Cleanup and the repair review show it). `total`: seconds of output.
fn progress_to(app: &AppHandle, path: &str, total: Option<f64>) -> impl Fn(f64) + 'static {
    let (a, which, total) = (app.clone(), path.to_string(), total.filter(|t| *t > 0.0));
    move |secs| {
        if let Some(t) = total {
            let pct = (secs / t * 100.0).clamp(0.0, 99.0).round() as u32;
            let _ = a.emit("edit-progress", serde_json::json!({ "path": which, "pct": pct }));
        }
    }
}

/// The copy Studio plays for smooth scrubbing (scrub.rs), or None when the clip
/// scrubs fine as it is. Made on first use, with "scrub-progress" { path, pct }.
#[tauri::command]
async fn smooth_copy(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("scrub");
    let total = library::probe(&path).ok().map(|p| p.duration).filter(|d| *d > 0.0);
    let (a, which) = (app.clone(), path.clone());
    let report = move |secs: f64| {
        if let Some(t) = total {
            let pct = (secs / t * 100.0).clamp(0.0, 99.0).round() as u32;
            let _ = a.emit("scrub-progress", serde_json::json!({ "path": which, "pct": pct }));
        }
    };
    blocking(move || scrub::smooth_copy(&dir, &path, report).map(|o| o.map(|p| p.to_string_lossy().into_owned()))).await
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
    let (nas_dir, nas_server) = { let s = settings_of(&app); (s.nas_dir, s.nas_server) };
    let on_nas = nas::rel_on_nas(&nas_dir, &path).is_some();
    if replace && health::on_network(&path) && !on_nas {
        return Err("This clip is on a network folder, which has no Recycle Bin, so it can't be replaced. Turn off \"Replace the original\" to save your edit as a new file.".into());
    }
    // Edits keep the clip's date: a trim stays where the clip was, not under today.
    let dates = library::dates_of(std::path::Path::new(&original));
    // Progress for Studio: seconds written so far, out of the edit's length.
    let report = progress_to(&app, &original, expected_secs.or_else(|| duration_of(&path)));
    let out = blocking(move || ffmpeg::with_progress(report, || f(&path))).await?;
    if !replace {
        if let Some(d) = dates {
            library::set_dates(std::path::Path::new(&out), d);
        }
        remember_edited(&app, &out);
        return Ok(out);
    }
    let o = original.clone();
    let final_path = if on_nas {
        blocking(move || nas::replace_on_nas(&nas_dir, &nas_server, &o, &out, expected_secs)).await?
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
    // Muted stretches or volume changes are in the separate tracks; the All audio
    // track other players use is rebuilt from them so it has the change too.
    let changed = !audio.is_empty();
    let remix = move |out: String| -> Result<String, String> {
        if changed {
            library::refresh_mix_track(&out)?;
        }
        Ok(out)
    };
    match crop.filter(|c| !c.is_whole()) {
        Some(c) => edit(app, path, replace, Some(end - start), move |p| library::trim_cropped(p, start, end, &audio, c).and_then(remix)).await,
        None => edit(app, path, replace, Some(end - start), move |p| library::trim(p, start, end, exact, &audio).and_then(remix)).await,
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

/// Linked folders and other recorders' folders for diagnostics: where each came
/// from, how many clips it has and the newest, and the ones that overlap (the
/// same folder written twice, or one holding the clips folder).
fn diag_folders(s: &Settings) -> String {
    let known = bench::recorders::clip_folders();
    let clips_dir = PathBuf::from(&s.clips_dir);
    let label_of = |p: &str| known.iter().find(|k| library::same_path(Path::new(&k.path), Path::new(p))).map(|k| k.label.clone());
    let summary = |p: &str| {
        let root = library::LinkedRoot { path: PathBuf::from(p), label: String::new() };
        let mut found = Vec::new();
        library::scan_linked(&root, &[clips_dir.clone()], &mut found);
        let newest = found.iter().map(|c| c.modified).max().and_then(chrono::DateTime::from_timestamp_millis);
        format!(
            "{} clip{}{}",
            found.len(),
            if found.len() == 1 { "" } else { "s" },
            newest.map(|t| format!(", newest {}", t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"))).unwrap_or_default()
        )
    };
    let mut lines = Vec::new();
    if s.linked_folders.is_empty() {
        lines.push("Linked: none".to_string());
    }
    for (i, f) in s.linked_folders.iter().enumerate() {
        let p = Path::new(f);
        let mut notes = Vec::new();
        if let Some(l) = label_of(f) {
            notes.push(format!("{l}'s folder"));
        }
        if !p.is_dir() {
            notes.push("missing".into());
        } else {
            notes.push(summary(f));
        }
        if clips_dir.starts_with(p) && !library::same_path(&clips_dir, p) {
            notes.push("holds the clips folder (skipped there)".into());
        }
        if s.linked_folders[..i].iter().any(|o| library::same_path(Path::new(o), p)) {
            notes.push("same folder as an earlier entry".into());
        }
        lines.push(format!("Linked: {f} ({})", notes.join(", ")));
    }
    for k in known.iter().filter(|k| k.exists && !s.linked_folders.iter().any(|f| library::same_path(Path::new(f), Path::new(&k.path)))) {
        lines.push(format!("Not linked: {} folder {} ({})", k.label, k.path, summary(&k.path)));
    }
    lines.join("\n")
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
            ("Folders", diag_folders(&s)),
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

/// The library the game finder works on: this PC, linked folders and, when it can be
/// reached, the NAS (a missing NAS would otherwise cost a network timeout per clip).
fn detect_library(s: &Settings) -> Vec<library::ClipFile> {
    let nas = if !s.nas_dir.is_empty() && Path::new(&s.nas_dir).is_dir() { nas::listing(&s.nas_dir) } else { Vec::new() };
    library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &nas, &linked_roots(s), &s.nas_dir)
}

#[tauri::command]
async fn game_suggestions(app: AppHandle) -> Result<Vec<game_detect::Suggestion>, String> {
    let s = settings_of(&app);
    blocking(move || {
        let all = detect_library(&s);
        // A clip looked at here may since have been freed up (only on the NAS now) or
        // moved: point at its NAS copy, or leave it out. A suggestion for a file that
        // isn't there showed no picture, and Show in folder and Move did nothing.
        let clips_dir = PathBuf::from(&s.clips_dir);
        Ok(game_detect::suggestions(&all, &s.detect_dismissed, &s.detect_folders, &s.not_game_folders)
            .into_iter()
            .filter_map(|mut g| {
                if Path::new(&g.path).is_file() {
                    return Some(g);
                }
                let rel = Path::new(&g.path).strip_prefix(&clips_dir).ok()?;
                let on_nas = Path::new(&s.nas_dir).join(rel);
                (!s.nas_dir.is_empty() && on_nas.is_file()).then(|| {
                    g.path = on_nas.to_string_lossy().into_owned();
                    g
                })
            })
            .collect())
    })
    .await
}

/// Move the chosen clips into their game's folder, on this PC and the NAS together
/// (logged, so Undo works like other moves).
#[tauri::command]
async fn apply_game_sort(app: AppHandle, picks: Vec<(String, String)>) -> Result<consolidate::Outcome, String> {
    move_into_folders(app, picks, "Sorted by game").await
}

/// Selected clips → one folder (an existing one or a new one), on this PC and the
/// NAS together, undoable like any other move.
#[tauri::command]
async fn move_to_folder(app: AppHandle, paths: Vec<String>, folder: String) -> Result<consolidate::Outcome, String> {
    let name = consolidate::safe_name(&folder);
    if name.trim().is_empty() {
        return Err("Give the folder a name.".into());
    }
    let label = format!("Moved to {name}");
    move_into_folders(app, paths.into_iter().map(|p| (p, name.clone())).collect(), &label).await
}

async fn move_into_folders(app: AppHandle, picks: Vec<(String, String)>, label: &str) -> Result<consolidate::Outcome, String> {
    let label = label.to_string();
    let s = settings_of(&app);
    if move_running() {
        return Err("Another move is still running. Try again when it's done.".into());
    }
    if !s.nas_dir.is_empty() {
        if nas::state().copying.is_some() {
            return Err("A clip is copying to the NAS right now. Try again once it's done.".into());
        }
        let root = PathBuf::from(&s.nas_dir);
        if !blocking(move || Ok(root.is_dir())).await? {
            return Err("Can't reach the NAS right now. Try again once it's back, so both copies move together.".into());
        }
    }
    let dir = moves_dir(&app)?;
    let a = app.clone();
    nas::hold(true);
    let out = blocking(move || {
        let mut sorted = game_detect::plan(Path::new(&s.clips_dir), Path::new(&s.nas_dir), &picks);
        sorted.plan.label = label.clone();
        let _ = std::fs::create_dir_all(&dir);
        let file_label: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let log = dir.join(format!("{}-{file_label}.jsonl", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let mut out = consolidate::run(&sorted.plan, &log, |done, total| {
            let _ = a.emit("move-progress", (done, total));
        });
        // Count clips, not files: a clip on both sides is one clip.
        out.moved = 0;
        for clip in &sorted.clips {
            if clip.from.iter().all(|p| !out.failed.iter().any(|(f, _)| *f == p.to_string_lossy())) {
                out.moved += 1;
                if let Some(old) = &clip.old_rel {
                    nas::clip_renamed(old, &clip.new_rel);
                }
            }
        }
        nas::refresh_listing(&s.nas_dir);
        Ok(out)
    })
    .await;
    nas::hold(false);
    let _ = app.emit("clips-changed", ());
    out
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DupeSet {
    paths: Vec<String>,
    /// "copy": byte for byte the same. "remux": the same recording in another format
    /// (OBS keeps the MKV next to the MP4 it remuxes to). "smaller": the same recording
    /// re-encoded smaller (an export next to its original).
    kind: &'static str,
}

/// Sets of library paths that hold the same clip: copies (checked byte for byte)
/// and remuxes (same name and recording, another format).
#[tauri::command]
async fn find_duplicates(app: AppHandle) -> Result<Vec<DupeSet>, String> {
    let s = settings_of(&app);
    // It reads and probes every clip: never while you play (Cleanup says so and tries later).
    if games::game_running() {
        return Err("game".into());
    }
    blocking(move || {
        let linked = linked_roots(&s);
        // Byte-for-byte copies: only files on this PC. A clip and its NAS copy are the
        // same clip on purpose (same name and format), and NAS files aren't read in full.
        let all = library::scan_all(PathBuf::from(&s.clips_dir).as_path(), &s.external_files, &nas::listing(&s.nas_dir), &linked, &s.nas_dir);
        let list: Vec<(String, u64)> = all
            .iter()
            .filter(|c| c.location != "nas" && !c.path.starts_with(r"\\") && !health::on_network(&c.path))
            .map(|c| (c.path.clone(), c.size))
            .collect();
        // Remuxes and smaller copies: clips only on the NAS too (OBS's MKV and MP4 often
        // end up there after freeing up space). They only compare names and details.
        let named: Vec<(String, u64)> = list
            .iter()
            .cloned()
            .chain(all.iter().filter(|c| c.location == "nas").map(|c| (c.path.clone(), c.size)))
            .collect();
        // Progress for Cleanup: files checked so far (the slow part is reading them).
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let copies = duplicates::find_with(&list, &mut |done, total| {
            if done == total || last.elapsed() >= std::time::Duration::from_millis(150) {
                last = std::time::Instant::now();
                let _ = app.emit("dupes-progress", (done, total));
            }
        });
        // A remux copies the streams untouched: nearly the same size (only the container
        // differs), length, picture and tracks. A re-encode of the same recording (an
        // export at a third of the size, say) isn't one: removing its original would lose quality.
        let sizes: std::collections::HashMap<&str, u64> = named.iter().map(|(p, s)| (p.as_str(), *s)).collect();
        let size = |p: &str| sizes.get(p).copied().unwrap_or(0);
        let same = |a: &str, b: &str| match (prepare::probe(a), prepare::probe(b)) {
            (Ok(x), Ok(y)) => duplicates::same_streams((&x, size(a)), (&y, size(b))),
            _ => false,
        };
        let in_copies: std::collections::HashSet<&String> = copies.iter().flatten().collect();
        let remuxes: Vec<Vec<String>> = duplicates::remuxes(&named, same).into_iter().filter(|s| !s.iter().all(|p| in_copies.contains(p))).collect();
        // The same recording re-encoded smaller (an export next to its original):
        // offered too, keeping the original by default so no quality is lost.
        let in_remuxes: std::collections::HashSet<&String> = remuxes.iter().flatten().collect();
        let recording = |a: &str, b: &str| match (prepare::probe(a), prepare::probe(b)) {
            (Ok(x), Ok(y)) => duplicates::same_recording(&x, &y),
            _ => false,
        };
        let smaller: Vec<Vec<String>> = duplicates::remuxes(&named, recording)
            .into_iter()
            .filter(|s| !s.iter().any(|p| in_copies.contains(p) || in_remuxes.contains(p)))
            .collect();
        Ok(copies
            .into_iter()
            .map(|paths| DupeSet { paths, kind: "copy" })
            .chain(remuxes.into_iter().map(|paths| DupeSet { paths, kind: "remux" }))
            .chain(smaller.into_iter().map(|paths| DupeSet { paths, kind: "smaller" }))
            .collect())
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MixRun {
    /// Clips on this PC that got the All audio track (their NAS copies follow)
    added_here: usize,
    failed_here: Vec<(String, String)>,
    /// The NAS app started on the clips only on the NAS
    nas_started: bool,
    nas_error: Option<String>,
}

/// Give older clips the All audio track: the ones on this PC here (their NAS copies
/// are updated by the sync, old versions kept in the NAS app's Trash), the ones only on
/// the NAS by the NAS app itself (no copying over the network). Only Hindsight's own
/// clips (library::needs_mix_track); Medal and OBS files are never touched.
#[tauri::command]
async fn add_mix_tracks(app: AppHandle) -> Result<MixRun, String> {
    if games::game_running() {
        return Err("Not while a game is running: try again after.".into());
    }
    let s = settings_of(&app);
    let a = app.clone();
    blocking(move || {
        let clips_dir = PathBuf::from(&s.clips_dir);
        let here: Vec<String> = library::scan_all(&clips_dir, &[], &[], &[], &s.nas_dir)
            .into_iter()
            .filter(|c| c.location == "local")
            .map(|c| c.path)
            .collect();
        let (mut added_here, mut failed_here) = (0, Vec::new());
        for (i, p) in here.iter().enumerate() {
            match library::add_mix_track(p) {
                Ok(true) => {
                    added_here += 1;
                    nas::edited(&s, std::path::Path::new(p), std::path::Path::new(p));
                }
                Ok(false) => {}
                Err(e) => failed_here.push((p.clone(), e)),
            }
            let _ = a.emit("mix-progress", (i + 1, here.len()));
        }
        // The NAS app does the rest, skipping the clips this PC just did (their copies come from here).
        let skip: Vec<String> = here.iter().filter_map(|p| library::rel_in(&clips_dir, p)).collect();
        let (nas_started, nas_error) = if s.nas_dir.is_empty() || s.nas_server.is_empty() {
            (false, None)
        } else {
            match nas::ask_app(&s.nas_server, "/api/mix-tracks", serde_json::json!({ "skip": skip })) {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e)),
            }
        };
        let _ = a.emit("clips-changed", ());
        Ok(MixRun { added_here, failed_here, nas_started, nas_error })
    })
    .await
}

/// How far the NAS app has got adding All audio tracks (None if it can't be asked).
#[tauri::command]
async fn mix_tracks_status(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    let server = settings_of(&app).nas_server;
    blocking(move || Ok(nas::app_get(&server, "/api/mix-tracks").ok())).await
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

/// Steam games named like `term`, for naming a new game folder. Nothing is sent
/// unless Online game logos is on (the same setting that lets game names go to Steam).
#[tauri::command]
async fn search_games(app: AppHandle, term: String) -> Result<Vec<String>, String> {
    if !settings_of(&app).online_game_logos {
        return Ok(Vec::new());
    }
    blocking(move || game_icons::search_steam(&term)).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GpuEncoders {
    gpu: String,
    /// "NVIDIA NVENC", "AMD AMF", "Intel Quick Sync", "Windows Media Foundation", "OpenH264 (CPU)"
    encoder: &'static str,
    /// Only NVIDIA has the native capture engine
    native: bool,
    /// "av1" | "hevc" | "h264" this GPU can encode
    supported: Vec<&'static str>,
    /// What "auto" records with
    auto: &'static str,
}

/// Which encoders this PC's GPU has (for the Encoder setting): NVIDIA, AMD, Intel or Windows'.
#[tauri::command]
async fn gpu_encoders() -> Result<GpuEncoders, String> {
    blocking(|| {
        let f = encoders::found();
        Ok(GpuEncoders {
            gpu: f.gpu.clone(),
            encoder: f.kind.label(),
            native: f.kind == encoders::Kind::Nvenc,
            supported: f.codecs.clone(),
            auto: encoders::effective("auto"),
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
    let a = app.clone();
    let res = blocking(move || {
        // A clip in the NAS folder (only on the NAS) is deleted there; the rest go
        // to the Recycle Bin, and their NAS copies (same game folder and name) go too.
        // In batches of 20, with "delete-progress" (done, total) after each.
        let total = paths.len();
        let mut done = 0;
        let progress = |done: usize| {
            let _ = a.emit("delete-progress", (done, total));
        };
        progress(0);
        let (on_nas, here): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| nas::rel_on_nas(&s.nas_dir, p).is_some());
        let clips_dir = PathBuf::from(&s.clips_dir);
        let mut res = library::DeleteResult::default();
        for batch in here.chunks(20) {
            let part = library::delete_clips(batch, &clips_dir, &s.external_files, &s.linked_folders, &s.nas_dir);
            for p in &part.deleted {
                if let Some(rel) = library::rel_in(&clips_dir, p)
                    && let Err(e) = nas::delete_on_nas(&s.nas_dir, &s.nas_server, &rel)
                {
                    res.failed.push((p.clone(), format!("Deleted from this PC, but {e}")));
                }
            }
            res.deleted.extend(part.deleted);
            res.failed.extend(part.failed);
            done += batch.len();
            progress(done);
        }
        for p in on_nas {
            let rel = nas::rel_on_nas(&s.nas_dir, &p).unwrap_or_default();
            match nas::delete_on_nas(&s.nas_dir, &s.nas_server, &rel) {
                Ok(_) => res.deleted.push(p),
                Err(e) => res.failed.push((p, e)),
            }
            done += 1;
            progress(done);
        }
        // A game folder in the clips folder left with nothing in it goes too (the
        // dialog said so when every clip in it was picked).
        let mut dirs: Vec<PathBuf> = res.deleted.iter().filter_map(|p| Path::new(p).parent().map(Path::to_path_buf)).collect();
        dirs.sort();
        dirs.dedup();
        for d in dirs {
            if d.starts_with(&clips_dir) && !library::same_path(&d, &clips_dir) {
                let _ = std::fs::remove_dir(&d); // only succeeds when empty
            }
        }
        Ok(res)
    })
    .await?;
    let _ = app.emit("clips-changed", ());
    Ok(res)
}

#[tauri::command]
async fn restore_clips(app: AppHandle, paths: Vec<String>) -> Result<usize, String> {
    let s = settings_of(&app);
    let n = blocking(move || {
        let (on_nas, here): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| nas::rel_on_nas(&s.nas_dir, p).is_some());
        let mut n = if here.is_empty() { 0 } else { library::restore_clips(&here)? };
        // Their NAS copies come back too.
        let clips_dir = PathBuf::from(&s.clips_dir);
        for p in &here {
            if let Some(rel) = library::rel_in(&clips_dir, p) {
                let _ = nas::undelete_on_nas(&s.nas_dir, &s.nas_server, &rel);
            }
        }
        for p in &on_nas {
            if nas::undelete_on_nas(&s.nas_dir, &s.nas_server, &nas::rel_on_nas(&s.nas_dir, p).unwrap_or_default())? {
                n += 1;
            }
        }
        Ok(n)
    })
    .await?;
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
            apply_log(&handle, &s);
            pin_monitor(&mut s, None);
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
                        (detect_library(&s), s.detect_folders.clone(), s.not_game_folders.clone())
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
                    library::set_nas_app(&s.nas_dir, &s.nas_server);
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
            session_end::start(handle.clone());

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
            find_game_in_folder,
move_anyway,
move_status,
stop_move,
smooth_copy,
get_settings,
            set_settings,
            screen_preview,
            nas_check,
            nas_confirm,
            nas_pair,
            nas_sign_in,
            nas_open_share,
            nas_saved_login,
            nas_forget_login,
            get_status,
            restart_capture,
            list_monitors,
            list_audio_inputs,
            save_clip,
            toggle_recording,
            list_clips,
            probe_file,
            known_lengths,
            search_games,
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
            add_mix_tracks,
            mix_tracks_status,
            find_duplicates,
            detect_status,
            detect_install,
            game_suggestions,
            apply_game_sort,
            move_to_folder,
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
            log_ui,
            open_log_folder,
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
