//! Auto-update from GitHub Releases.
//!
//! - Checks shortly after launch and every 6 hours.
//! - Updates are signed; the app only accepts files signed with Hindsight's key
//!   (the public half is in tauri.conf.json), so a tampered download is rejected.
//! - Never interrupts a recording or a game:
//!   * Right after launch (nothing captured yet), a found update installs
//!     immediately unless a recording or game is running.
//!   * Otherwise it downloads in the background and installs when you click
//!     "Restart to update" or the next time Hindsight quits.

use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub current: String,
    /// "idle" | "checking" | "downloading" | "ready" | "up-to-date" | "error"
    pub stage: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    state: Mutex<UpdateState>,
    ready: Mutex<Option<(Update, Vec<u8>)>>,
}

fn set(app: &AppHandle, f: impl FnOnce(&mut UpdateState)) {
    let u = app.state::<Updates>();
    let snapshot = {
        let mut s = u.state.lock().unwrap();
        f(&mut s);
        s.clone()
    };
    let _ = app.emit("update-state", snapshot);
}

pub fn state(app: &AppHandle) -> UpdateState {
    let u = app.state::<Updates>();
    let mut s = u.state.lock().unwrap().clone();
    s.current = app.package_info().version.to_string();
    s
}

pub fn has_ready_update(app: &AppHandle) -> bool {
    app.state::<Updates>().ready.lock().unwrap().is_some()
}

/// Check, and download if `download` is set. Returns the new version, if any.
pub async fn check(app: &AppHandle, download: bool) -> Result<Option<String>, String> {
    if has_ready_update(app) {
        return Ok(state(app).version);
    }
    set(app, |s| {
        s.stage = "checking".into();
        s.error = None;
    });
    let result: Result<Option<String>, String> = async {
        let updater = app.updater().map_err(|e| e.to_string())?;
        let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
            set(app, |s| s.stage = "up-to-date".into());
            return Ok(None);
        };
        let version = update.version.clone();
        set(app, |s| {
            s.version = Some(version.clone());
            s.notes = update.body.clone();
            s.stage = if download { "downloading" } else { "available" }.into();
        });
        if download {
            // download() also checks the signature and fails on a mismatch.
            let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
            *app.state::<Updates>().ready.lock().unwrap() = Some((update, bytes));
            set(app, |s| s.stage = "ready".into());
            crate::log!("[rewind] update {version} downloaded and signature verified");
        }
        Ok(Some(version))
    }
    .await;
    if let Err(e) = &result {
        crate::log!("[rewind] update check failed: {e}");
        set(app, |s| {
            s.stage = "error".into();
            s.error = Some(e.clone());
        });
    }
    result
}

/// Install the downloaded update (Hindsight closes and reopens on the new version).
pub fn install(app: &AppHandle) -> Result<(), String> {
    if std::env::var("REWIND_UPDATE_DRYRUN").is_ok() {
        crate::log!("[rewind] dry run: would install the update now");
        return Ok(());
    }
    let Some((update, bytes)) = app.state::<Updates>().ready.lock().unwrap().take() else {
        return Err("No update has been downloaded yet.".into());
    };
    // Finish any recording and stop ffmpeg before the installer runs.
    app.state::<crate::AppState>().engine.shutdown();
    update.install(bytes).map_err(|e| e.to_string())
}

/// Background loop: check after launch, then every 6 hours.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio_sleep(Duration::from_secs(8)).await;
        let mut first = true;
        loop {
            let auto = app.state::<crate::AppState>().settings.lock().unwrap().auto_update;
            if let Ok(Some(_)) = check(&app, auto).await
                && auto
                && first
                && has_ready_update(&app)
                && safe_to_restart(&app)
            {
                crate::log!("[rewind] installing update found at launch");
                let _ = install(&app);
            }
            first = false;
            tokio_sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

/// Not recording and no game in front.
pub fn safe_to_restart(app: &AppHandle) -> bool {
    let st = app.state::<crate::AppState>().engine.status();
    // Not while a clip is copying to the NAS or clips are moving into Hindsight either:
    // nothing would be lost (copies are checked before they count), but it'd start over.
    let nas = crate::nas::state();
    !st.recording && st.game.is_none() && crate::engine::saves_in_progress() == 0 && nas.copying.is_none() && !crate::move_running()
}

async fn tokio_sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

/// What changed in a version: its release notes on GitHub, and the day it came out.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNotes {
    pub version: String,
    /// RFC 3339, e.g. "2026-11-25T08:02:02Z"
    pub date: String,
    pub notes: String,
}

static NOTES: Mutex<Option<ReleaseNotes>> = Mutex::new(None);

/// The release notes for `version` (the one installed), read once from GitHub and kept.
pub fn whats_new(version: &str) -> Result<ReleaseNotes, String> {
    if let Some(n) = NOTES.lock().unwrap().as_ref().filter(|n| n.version == version) {
        return Ok(n.clone());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Hindsight")
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("https://api.github.com/repos/aattiaibrahim/hindsight/releases/tags/v{version}");
    let r = client.get(url).header("Accept", "application/vnd.github+json").send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("GitHub said {}", r.status()));
    }
    let v: serde_json::Value = r.json().map_err(|e| e.to_string())?;
    let notes = ReleaseNotes {
        version: version.to_string(),
        date: v["published_at"].as_str().unwrap_or_default().to_string(),
        notes: changes_only(v["body"].as_str().unwrap_or_default()),
    };
    *NOTES.lock().unwrap() = Some(notes.clone());
    Ok(notes)
}

/// The release text up to its first heading: what changed, without the "### Install" part.
fn changes_only(body: &str) -> String {
    let body = body.replace("\r\n", "\n");
    let end = body.find("\n#").unwrap_or(body.len());
    let head = &body[..end];
    if head.trim_start().starts_with('#') { String::new() } else { head.trim().to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_without_the_install_part() {
        let body = "New game in Clips.\r\n\r\n### Install\r\nDownload it.";
        assert_eq!(changes_only(body), "New game in Clips.");
        assert_eq!(changes_only("Just this."), "Just this.");
        assert_eq!(changes_only("### Install\nDownload it."), "");
    }
}
