//! The Performance test (PresentMon, other recorders' hotkeys) is Windows-only.

pub mod run {
    use serde::{Deserialize, Serialize};
    use tauri::AppHandle;

    #[derive(Deserialize, Clone)]
    pub struct Options {}

    #[derive(Serialize, Clone)]
    pub struct Report {}

    pub fn last_report() -> Option<Report> {
        None
    }

    pub fn is_running() -> bool {
        false
    }

    pub fn cancel() {}

    pub fn start(_app: AppHandle, _opts: Options) -> Result<(), String> {
        Err("The performance test is only on Windows.".into())
    }
}

pub mod recorders {
    use serde::Serialize;

    /// Where another recorder saves its clips.
    #[derive(Serialize, Clone, Debug)]
    #[serde(rename_all = "camelCase")]
    pub struct ClipFolder {
        pub label: String,
        pub path: String,
        pub exists: bool,
    }

    /// OBS's default folder on this system, if it's there (others are phase 4).
    pub fn clip_folders() -> Vec<ClipFolder> {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return Vec::new() };
        let videos = if cfg!(target_os = "macos") { home.join("Movies") } else { home.join("Videos") };
        vec![ClipFolder { label: "OBS".into(), exists: videos.is_dir(), path: videos.to_string_lossy().into_owned() }]
    }
}
