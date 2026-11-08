use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub clips_dir: String,
    /// "auto" (best the GPU can do) | "av1" | "hevc" | "h264"
    pub encoder: String,
    /// NVENC constant-quality value (lower = better, bigger files)
    pub quality: u32,
    pub fps: u32,
    /// Clip height: 0 = same as the monitor, else 720 / 1080 / 1440 / 2160
    pub output_height: u32,
    pub buffer_seconds: u32,
    /// Keep the replay buffer in memory (no SSD writes) instead of on disk
    pub buffer_in_ram: bool,
    /// "native" (Windows Graphics Capture + NVENC, default) or "ffmpeg" (compatibility)
    pub capture_engine: String,
    /// "primary" | "cursor" | "fixed"
    pub monitor_mode: String,
    /// Device name like \\.\DISPLAY1, used when monitor_mode == "fixed"
    pub fixed_monitor: String,
    pub draw_cursor: bool,
    /// Part of the screen to record, as fractions of the monitor; None = all of it.
    pub capture_area: Option<Area>,
    pub hotkey: String,
    /// Start / stop a full recording
    pub record_hotkey: String,
    pub buffer_enabled: bool,
    pub launch_on_startup: bool,
    /// Download updates in the background and install them when safe
    pub auto_update: bool,
    pub start_minimized: bool,
    pub keep_original_after_remux: bool,
    /// Trims / exports replace the file you edited (original goes to the Recycle Bin)
    pub replace_original_on_edit: bool,
    /// Videos opened from outside the clips folder
    pub external_files: Vec<String>,
    pub desktop_audio: bool,
    /// Record Discord on its own track (and leave it out of desktop audio)
    pub discord_track: bool,
    pub mic_enabled: bool,
    pub mic_mono: bool,
    /// "" = Windows default microphone
    pub mic_device: String,
    /// Microphone volume while recording, in dB (-20..+20, 0 = as Windows gives it).
    #[serde(default)]
    pub mic_gain_db: f64,
    /// Look of the app: "tinted" | "midnight" | "graphite" | "oled" (an old "light" becomes "midnight")
    pub theme: String,
    /// Highlight colour: "winblue" (default), "system" (follows Windows), "cyan", "rose", "lime", "amber", "violet", "emerald", "gold", "pink", "teal", "mono", "green"
    pub accent: String,
    /// Turn off fades and motion in the interface
    pub reduce_motion: bool,
    /// Play a short chime when a clip or recording is saved (and a low one if it fails)
    pub save_sound: bool,
    /// Network folder (e.g. \\nas\clips) that saved clips are copied to in the background; "" = off
    pub nas_dir: String,
    /// Delete the copy on this PC once a clip is safely on the NAS
    pub nas_remove_local: bool,
    /// Other recorders' folders (Medal, OBS, NVIDIA, any folder) shown in the library in place
    pub linked_folders: Vec<String>,
    /// The folder spelling picked when merging a game's folders (game key → name); new clips use it
    pub folder_names: std::collections::BTreeMap<String, String>,
    /// Games whose folder spellings the user chose to keep separate (game keys)
    pub similar_dismissed: Vec<String>,
    /// Cleanup: clips the user chose to keep (path → size then), hidden until the next cleanup check
    pub cleanup_kept: std::collections::BTreeMap<String, u64>,
    /// Show the Performance page (benchmark against other recorders)
    pub benchmark_mode: bool,
    /// Where Upload sends clips: "catbox" | "litterbox" | "discord" | "streamable"
    pub upload_host: String,
    /// How long Litterbox keeps a clip: "1h" | "12h" | "24h" | "72h"
    pub litterbox_time: String,
    /// Upload every newly saved clip in the background (link copied when done)
    pub auto_upload: bool,
    /// After uploading to Catbox / Litterbox / Streamable, also post the link to the Discord webhook
    pub post_link_to_discord: bool,
    pub streamable_email: String,
    /// The saved Discord webhook's name (just for display; the address itself is a secret)
    pub discord_webhook_name: String,
    /// Ask before uploading (a link is public to anyone who has it)
    pub confirm_uploads: bool,
    /// Check clips for damage in the background (only while no game is in front)
    pub health_check: bool,
    /// Make every clip's thumbnail and details ahead of time (only while no game is running)
    pub prepare_thumbnails: bool,
    /// Find the game in clips from folders that don't say (OBS, Desktop…) from the picture (downloads a model)
    pub game_detect: bool,
    /// Clips whose game suggestion was turned down
    pub detect_dismissed: Vec<String>,
    /// Clips taken off "To trim" ("Keep whole")
    pub keep_whole: Vec<String>,
    /// Game logos from Steam (only the game's name is sent), else the game's program icon
    pub online_game_logos: bool,
}

/// A rectangle of the screen, as fractions (0..1) of the monitor.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    /// Kept inside the screen and at least 5% each way; None for the whole screen.
    pub fn cleaned(self) -> Option<Area> {
        if ![self.x, self.y, self.w, self.h].iter().all(|v| v.is_finite()) {
            return None;
        }
        let w = self.w.clamp(0.05, 1.0);
        let h = self.h.clamp(0.05, 1.0);
        let a = Area { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h };
        let whole = a.x <= 0.002 && a.y <= 0.002 && a.w >= 0.998 && a.h >= 0.998;
        (!whole).then_some(a)
    }

    /// In pixels of a `width` × `height` screen: (x, y, w, h), even sizes, inside the screen.
    pub fn pixels(&self, width: i32, height: i32) -> (i32, i32, i32, i32) {
        let even = |v: f64| ((v.round() as i32) / 2 * 2).max(2);
        let w = even(self.w * width as f64).min(width / 2 * 2);
        let h = even(self.h * height as f64).min(height / 2 * 2);
        let x = ((self.x * width as f64).round() as i32).clamp(0, width - w);
        let y = ((self.y * height as f64).round() as i32).clamp(0, height - h);
        (x, y, w, h)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            clips_dir: String::new(),
            encoder: "auto".into(),
            quality: 24,
            fps: 60,
            output_height: 0,
            buffer_seconds: 120,
            buffer_in_ram: true,
            capture_engine: "native".into(),
            monitor_mode: "primary".into(),
            fixed_monitor: String::new(),
            draw_cursor: true,
            capture_area: None,
            hotkey: "Alt+F10".into(),
            record_hotkey: "Alt+F9".into(),
            buffer_enabled: true,
            launch_on_startup: false,
            auto_update: true,
            start_minimized: true,
            keep_original_after_remux: true,
            replace_original_on_edit: false,
            external_files: Vec::new(),
            desktop_audio: true,
            discord_track: true,
            mic_enabled: true,
            mic_mono: true,
            mic_device: String::new(),
            mic_gain_db: 0.0,
            theme: "tinted".into(),
            accent: "winblue".into(),
            reduce_motion: false,
            save_sound: true,
            nas_dir: String::new(),
            nas_remove_local: false,
            linked_folders: Vec::new(),
            folder_names: std::collections::BTreeMap::new(),
            similar_dismissed: Vec::new(),
            cleanup_kept: std::collections::BTreeMap::new(),
            benchmark_mode: false,
            upload_host: "catbox".into(),
            litterbox_time: "72h".into(),
            auto_upload: false,
            post_link_to_discord: false,
            streamable_email: String::new(),
            discord_webhook_name: String::new(),
            confirm_uploads: true,
            health_check: true,
            prepare_thumbnails: true,
            game_detect: false,
            detect_dismissed: Vec::new(),
            keep_whole: Vec::new(),
            online_game_logos: true,
        }
    }
}

impl Settings {
    /// Pull every value into a range the engine can actually use.
    pub fn sanitized(mut self) -> Settings {
        let d = Settings::default();
        if !["auto", "av1", "hevc", "h264"].contains(&self.encoder.as_str()) {
            self.encoder = d.encoder;
        }
        if !["native", "ffmpeg"].contains(&self.capture_engine.as_str()) {
            self.capture_engine = d.capture_engine.clone();
        }
        if !["primary", "cursor", "fixed"].contains(&self.monitor_mode.as_str()) {
            self.monitor_mode = d.monitor_mode;
        }
        self.quality = self.quality.clamp(10, 45);
        self.fps = self.fps.clamp(15, 240);
        self.buffer_seconds = self.buffer_seconds.clamp(5, 1800);
        self.mic_gain_db = if self.mic_gain_db.is_finite() { (self.mic_gain_db.clamp(-20.0, 20.0) * 2.0).round() / 2.0 } else { 0.0 };
        if ![0, 720, 1080, 1440, 2160].contains(&self.output_height) {
            self.output_height = 0;
        }
        self.capture_area = self.capture_area.and_then(Area::cleaned);
        if self.hotkey.trim().is_empty() {
            self.hotkey = d.hotkey;
        }
        if self.record_hotkey.trim().is_empty() {
            self.record_hotkey = d.record_hotkey;
        }
        self.external_files.dedup();
        self.nas_dir = self.nas_dir.trim().trim_end_matches(['\\', '/']).to_string();
        if self.nas_dir.is_empty() {
            self.nas_remove_local = false;
        }
        if !["catbox", "litterbox", "discord", "streamable"].contains(&self.upload_host.as_str()) {
            self.upload_host = d.upload_host.clone();
        }
        if !["1h", "12h", "24h", "72h"].contains(&self.litterbox_time.as_str()) {
            self.litterbox_time = d.litterbox_time.clone();
        }
        self.streamable_email = self.streamable_email.trim().to_string();
        self.linked_folders.retain(|f| !f.trim().is_empty());
        self.linked_folders.dedup();
        self.folder_names.retain(|k, v| !k.is_empty() && !v.trim().is_empty());
        self.similar_dismissed.sort();
        self.similar_dismissed.dedup();
        if self.theme == "light" {
            self.theme = "midnight".into();
        }
        if !["tinted", "midnight", "graphite", "oled"].contains(&self.theme.as_str()) {
            self.theme = d.theme.clone();
        }
        if !["winblue", "system", "cyan", "rose", "lime", "amber", "violet", "emerald", "gold", "pink", "teal", "mono", "green"].contains(&self.accent.as_str()) {
            self.accent = d.accent.clone();
        }
        self
    }
}

/// Missing or broken settings fall back to defaults (a broken file is kept as .bak).
pub fn load(path: &Path) -> Settings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(s) => s.sanitized(),
        Err(_) => {
            let _ = std::fs::copy(path, path.with_extension("json.bak"));
            Settings::default()
        }
    }
}

/// Write to a temp file first so a crash mid-save can't corrupt settings.
pub fn save(path: &Path, s: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_bad_values() {
        let s = Settings { fps: 9999, quality: 0, buffer_seconds: 0, encoder: "xvid".into(), hotkey: " ".into(), ..Default::default() }
            .sanitized();
        assert_eq!((s.fps, s.quality, s.buffer_seconds), (240, 10, 5));
        assert_eq!(s.encoder, "auto");
        assert_eq!(s.hotkey, "Alt+F10");
    }

    #[test]
    fn defaults_have_mic_on_and_mono() {
        let s = Settings::default();
        assert!(s.mic_enabled && s.mic_mono && s.desktop_audio);
    }

    #[test]
    fn broken_file_falls_back_and_is_kept() {
        let dir = std::env::temp_dir().join(format!("rewind-test-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, "{ not json").unwrap();
        let s = load(&p);
        assert_eq!(s.hotkey, "Alt+F10");
        assert!(p.with_extension("json.bak").exists());
        save(&p, &s).unwrap();
        assert_eq!(load(&p).hotkey, "Alt+F10");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
