//! Noticing Windows shutting down or signing out. Elsewhere (logind on Linux,
//! NSWorkspace on macOS) it's phase 4; the app still closes its files on quit.

use tauri::AppHandle;

pub fn start(_app: AppHandle) {}
