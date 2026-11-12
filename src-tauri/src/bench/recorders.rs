//! Reads each recorder's own capture settings (resolution, FPS, codec,
//! quality, buffer length), so the test can show whether it compared like
//! with like. Only the settings below are read; stream keys and accounts
//! are never touched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::HSTRING;

use super::measure::App;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecorderSettings {
    pub app: App,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub codec: Option<String>,
    pub quality: Option<String>,
    pub buffer_secs: Option<u32>,
    /// Where these came from, or why they couldn't be read.
    pub source: String,
}

impl RecorderSettings {
    fn empty(app: App, source: &str) -> Self {
        RecorderSettings { app, width: None, height: None, fps: None, codec: None, quality: None, buffer_secs: None, source: source.into() }
    }
}

/// Hindsight's own: its settings plus the size it's capturing at.
pub fn rewind(s: &crate::settings::Settings, screen: (i32, i32)) -> RecorderSettings {
    let (sw, sh) = (screen.0.max(0) as u32, screen.1.max(0) as u32);
    let (width, height) = if s.output_height == 0 || sh == 0 {
        (sw, sh)
    } else {
        let h = s.output_height;
        (((h as f64 * sw as f64 / sh as f64) / 2.0).round() as u32 * 2, h)
    };
    RecorderSettings {
        app: App::Rewind,
        width: (width > 0).then_some(width),
        height: (height > 0).then_some(height),
        fps: Some(s.fps as f64),
        codec: Some(match s.encoder.as_str() {
            "hevc" => "HEVC (NVENC)".into(),
            "h264" => "H.264 (NVENC)".into(),
            _ => "AV1 (NVENC)".into(),
        }),
        quality: Some(format!("CQ {}", s.quality)),
        buffer_secs: Some(s.buffer_seconds),
        source: "Hindsight settings".into(),
    }
}

/// `[section] key=value` files (OBS). Keys are "section.key".
fn ini(path: &Path) -> Option<HashMap<String, String>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            out.insert(format!("{section}.{k}"), v.to_string());
        }
    }
    Some(out)
}

fn codec_name(encoder_id: &str) -> String {
    let e = encoder_id.to_ascii_lowercase();
    let hw = if e.contains("nvenc") {
        "NVENC"
    } else if e.contains("x264") || e == "obs_x264" {
        "CPU x264"
    } else if e.contains("amf") {
        "AMD"
    } else if e.contains("qsv") {
        "Intel QSV"
    } else {
        "?"
    };
    let codec = if e.contains("av1") {
        "AV1"
    } else if e.contains("hevc") || e.contains("265") {
        "HEVC"
    } else {
        "H.264"
    };
    format!("{codec} ({hw})")
}

pub fn obs() -> RecorderSettings {
    let Some(appdata) = std::env::var_os("APPDATA") else { return RecorderSettings::empty(App::Obs, "Couldn't find OBS settings") };
    obs_from(&PathBuf::from(appdata).join("obs-studio"))
}

/// The current OBS profile: (name, its folder, its basic.ini).
fn obs_profile(dir: &Path) -> Option<(String, PathBuf, HashMap<String, String>)> {
    // The current profile is in user.ini (OBS 31+) or global.ini.
    let profile = ["user.ini", "global.ini"]
        .iter()
        .filter_map(|f| ini(&dir.join(f)))
        .find_map(|g| g.get("Basic.ProfileDir").or_else(|| g.get("Basic.Profile")).cloned())
        .or_else(|| {
            std::fs::read_dir(dir.join("basic").join("profiles")).ok()?.flatten().find(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned())
        })?;
    let pdir = dir.join("basic").join("profiles").join(&profile);
    let b = ini(&pdir.join("basic.ini"))?;
    Some((profile, pdir, b))
}

pub fn obs_from(dir: &Path) -> RecorderSettings {
    let mut r = RecorderSettings::empty(App::Obs, "Couldn't read OBS settings");
    let Some((profile, pdir, b)) = obs_profile(dir) else { return r };
    let get = |k: &str| b.get(k).map(String::as_str);
    let num = |k: &str| get(k).and_then(|v| v.parse::<u32>().ok());

    r.width = num("Video.OutputCX");
    r.height = num("Video.OutputCY");
    r.fps = match get("Video.FPSType").unwrap_or("0") {
        "1" => get("Video.FPSInt").and_then(|v| v.parse().ok()),
        "2" => match (get("Video.FPSNum").and_then(|v| v.parse::<f64>().ok()), get("Video.FPSDen").and_then(|v| v.parse::<f64>().ok())) {
            (Some(n), Some(d)) if d > 0.0 => Some(n / d),
            _ => None,
        },
        _ => get("Video.FPSCommon").and_then(|v| v.split_whitespace().next()?.parse().ok()).or(Some(30.0)),
    };

    if get("Output.Mode") == Some("Advanced") {
        let enc = match get("AdvOut.RecEncoder") {
            Some("none") | None => get("AdvOut.Encoder").unwrap_or("obs_x264"),
            Some(e) => e,
        };
        r.codec = Some(codec_name(enc));
        r.buffer_secs = num("AdvOut.RecRBTime").or(Some(20));
        if get("AdvOut.RecRescale") == Some("true") {
            if let Some((w, h)) = get("AdvOut.RecRescaleRes").and_then(|v| v.split_once('x')) {
                r.width = w.parse().ok();
                r.height = h.parse().ok();
            }
        }
        // Only the rate-control keys of the recording encoder's settings.
        if let Some(j) = std::fs::read_to_string(pdir.join("recordEncoder.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            let rc = j.get("rate_control").and_then(|v| v.as_str()).unwrap_or("CBR");
            r.quality = Some(match rc {
                "CQP" => format!("CQP {}", j.get("cqp").and_then(|v| v.as_i64()).unwrap_or(20)),
                "CRF" => format!("CRF {}", j.get("crf").and_then(|v| v.as_i64()).unwrap_or(23)),
                _ => format!("{rc} {} kbps", j.get("bitrate").and_then(|v| v.as_i64()).unwrap_or(2500)),
            });
        }
    } else {
        let quality = get("SimpleOutput.RecQuality").unwrap_or("Stream");
        let enc = if quality == "Stream" { get("SimpleOutput.StreamEncoder") } else { get("SimpleOutput.RecEncoder") };
        r.codec = Some(codec_name(enc.unwrap_or("x264")));
        r.quality = Some(if quality == "Stream" { format!("Same as stream, {} kbps", get("SimpleOutput.VBitrate").unwrap_or("2500")) } else { quality.to_string() });
        r.buffer_secs = num("SimpleOutput.RecRBTime").or(Some(20));
    }
    r.source = format!("OBS profile \"{profile}\"");
    r
}

fn reg_bytes(path: &str, name: &str) -> Option<Vec<u8>> {
    unsafe {
        let mut buf = [0u8; 16];
        let mut len = buf.len() as u32;
        let ok = RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            &HSTRING::from(name),
            RRF_RT_REG_BINARY | RRF_RT_REG_DWORD,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        (ok.is_ok() && len >= 4).then(|| buf[..len as usize].to_vec())
    }
}

/// A 4-byte little-endian registry value (REG_BINARY or REG_DWORD).
pub(crate) fn reg_u32(path: &str, name: &str) -> Option<u32> {
    reg_bytes(path, name).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// NVIDIA keeps Instant Replay's settings as little-endian binary values.
pub fn nvidia() -> RecorderSettings {
    const KEY: &str = r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS";
    let mut r = RecorderSettings::empty(App::Nvidia, "NVIDIA Instant Replay settings (codec and bitrate aren't stored where Hindsight can read them)");
    let u32_of = |name| reg_bytes(KEY, name).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    r.width = u32_of("VideoWidth").filter(|v| *v > 0);
    r.height = u32_of("VideoHeight").filter(|v| *v > 0);
    r.buffer_secs = u32_of("DVRBufferLen").filter(|v| *v > 0);
    r.fps = reg_bytes(KEY, "RecordingFPS").map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).filter(|v| (1.0..=1000.0).contains(v));
    if r.width.is_none() && r.fps.is_none() {
        r.source = "Couldn't read NVIDIA Instant Replay settings".into();
    }
    r
}

fn find_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find_key(x, key))),
        serde_json::Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
        _ => None,
    }
}

pub fn medal() -> RecorderSettings {
    let mut r = RecorderSettings::empty(App::Medal, "Couldn't read Medal settings");
    let Some(appdata) = std::env::var_os("APPDATA") else { return r };
    let path = PathBuf::from(appdata).join("Medal").join("store").join("settings.json");
    let Some(j) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return r };
    r.codec = find_key(&j, "selectedCodec").and_then(|v| v.as_str()).map(|c| match c.to_ascii_uppercase().as_str() {
        "H265" | "HEVC" => "HEVC".to_string(),
        "H264" => "H.264".to_string(),
        other => other.to_string(),
    });
    r.quality = find_key(&j, "quality").and_then(|q| q.get("bitrate")).and_then(|v| v.as_str()).map(|b| format!("{b}bps"));
    r.source = "Medal settings (resolution, FPS and clip length aren't stored where Hindsight can read them)".into();
    r
}

/// Where another recorder saves its clips, for showing them in Hindsight's library.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClipFolder {
    /// "Medal" | "OBS" | "NVIDIA"
    pub label: String,
    pub path: String,
    pub exists: bool,
}

fn reg_string(path: &str, name: &str) -> Option<String> {
    unsafe {
        let mut buf = vec![0u8; 2048];
        let mut len = buf.len() as u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            &HSTRING::from(name),
            RRF_RT_REG_BINARY | RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
        .ok()
        .ok()?;
        let wide: Vec<u16> = buf[..len as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        Some(String::from_utf16_lossy(&wide)).filter(|s| !s.is_empty())
    }
}

/// Other recorders and where they keep clips, so their clips can show in the
/// library and be moved into Hindsight. Read from each app's own settings
/// where it has them, else its default folder. Only folders that exist are
/// offered. To support another recorder, add it to `CATALOG` (or, if it keeps
/// its folder in a settings file, next to Medal / OBS / NVIDIA below).
///
/// (label, folder under the Videos folder; the first that exists wins)
const CATALOG: &[(&str, &[&str])] = &[
    // Ascent keeps clips in Clips; Downloads holds copies (shown as duplicates in Cleanup).
    ("Ascent", &[r"Ascent\Clips", "Ascent"]),
    ("AMD ReLive", &["Radeon ReLive", "AMD"]),
    ("Xbox Game Bar", &["Captures"]),
    ("Outplayed", &[r"Overwolf\Outplayed"]),
    ("Insights Capture", &[r"Overwolf\Insights Capture"]),
    ("Overwolf", &["Overwolf"]),
    ("SteelSeries Moments", &[r"SteelSeries\Moments", "SteelSeries Moments"]),
    ("Streamlabs", &["Streamlabs", "Streamlabs OBS"]),
    ("Plays", &["Plays"]),
    ("Clipchamp", &["Clipchamp"]),
];

/// The user's Videos folder (it can be moved, e.g. to another drive).
fn videos_dir() -> Option<PathBuf> {
    reg_string(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders", "My Video")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Videos")))
}

/// Recorders' clip folders, from their own settings or their default folder.
pub fn clip_folders() -> Vec<ClipFolder> {
    let mut out = Vec::new();
    let mut push = |label: &str, path: Option<String>| {
        if let Some(p) = path {
            let p = crate::library::tidy_path(&p);
            if !p.is_empty() && !out.iter().any(|f: &ClipFolder| f.path.eq_ignore_ascii_case(&p)) {
                out.push(ClipFolder { label: label.into(), exists: Path::new(&p).is_dir(), path: p });
            }
        }
    };
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let medal = appdata
        .as_ref()
        .and_then(|a| std::fs::read_to_string(a.join("Medal").join("store").join("settings.json")).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|j| find_key(&j, "clipFolder").and_then(|v| v.as_str()).map(str::to_string))
        .or_else(|| Path::new(r"C:\Medal").is_dir().then(|| r"C:\Medal".to_string()));
    push("Medal", medal);
    let obs = appdata.as_ref().and_then(|a| obs_profile(&a.join("obs-studio"))).and_then(|(_, _, b)| {
        let key = if b.get("Output.Mode").map(String::as_str) == Some("Advanced") { "AdvOut.RecFilePath" } else { "SimpleOutput.FilePath" };
        b.get(key).cloned()
    });
    push("OBS", obs);
    push("NVIDIA", reg_string(r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS", "DefaultPathW"));
    // Xbox Game Bar's folder can be moved: Windows keeps it as a known folder.
    push("Xbox Game Bar", reg_string(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders", "{EDC0FE71-98D8-4F4A-B920-C8DC133CB165}").map(|p| {
        let profile = std::env::var("USERPROFILE").unwrap_or_default();
        p.replace("%USERPROFILE%", &profile)
    }));
    if let Some(videos) = videos_dir() {
        for (label, subs) in CATALOG {
            if let Some(dir) = subs.iter().map(|s| videos.join(s)).find(|d| d.is_dir()) {
                push(label, Some(dir.to_string_lossy().into_owned()));
            }
        }
    }
    out.retain(|f| f.exists || ["Medal", "OBS", "NVIDIA"].contains(&f.label.as_str()));
    out
}

#[cfg(test)]
mod catalog_tests {
    /// What this PC has: cargo test live_recorder_folders -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_recorder_folders() {
        for f in super::clip_folders() {
            println!("{}: {} (exists: {})", f.label, f.path, f.exists);
        }
    }
}

pub fn read(app: App) -> RecorderSettings {
    match app {
        App::Obs => obs(),
        App::Nvidia => nvidia(),
        App::Medal => medal(),
        App::Rewind => RecorderSettings::empty(App::Rewind, ""),
    }
}

/// Warn where a recorder captures at a different size or frame rate than Hindsight.
pub fn mismatches(all: &[RecorderSettings]) -> Vec<String> {
    let mut w = Vec::new();
    let Some(ours) = all.iter().find(|s| s.app == App::Rewind) else { return w };
    for s in all.iter().filter(|s| s.app != App::Rewind) {
        let name = super::run::name(s.app);
        let mut diffs = Vec::new();
        if let (Some(w1), Some(h1), Some(w2), Some(h2)) = (s.width, s.height, ours.width, ours.height) {
            if (w1, h1) != (w2, h2) {
                diffs.push(format!("{w1}×{h1} instead of {w2}×{h2}"));
            }
        }
        if let (Some(a), Some(b)) = (s.fps, ours.fps) {
            if (a - b).abs() > 0.5 {
                diffs.push(format!("{a:.0} fps instead of {b:.0}"));
            }
        }
        if !diffs.is_empty() {
            w.push(format!("{name} records at {} — not the same work as Hindsight, so the comparison isn't like for like. Match the settings and run again.", diffs.join(" and ")));
        }
        if let (Some(a), Some(b)) = (s.buffer_secs, ours.buffer_secs) {
            if a != b {
                w.push(format!("{name} keeps a {a} s buffer and Hindsight {b} s, so their RAM numbers aren't comparable (a buffer in RAM grows with its length)."));
            }
        }
        if s.codec.as_deref().is_some_and(|c| c.contains("CPU")) {
            w.push(format!("{name} encodes on the CPU (x264), not the GPU's encoder, so its cost is mostly CPU."));
        }
        if s.width.is_none() && s.fps.is_none() {
            w.push(format!("Hindsight couldn't read {name}'s resolution and frame rate. Check they match Hindsight's ({}×{} at {} fps).",
                ours.width.unwrap_or(0), ours.height.unwrap_or(0), ours.fps.unwrap_or(0.0)));
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_obs_advanced_profile() {
        let dir = std::env::temp_dir().join(format!("rewind-obs-test-{}", std::process::id()));
        let p = dir.join("basic").join("profiles").join("Gaming");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(dir.join("user.ini"), "[Basic]\nProfile=Gaming\nProfileDir=Gaming\n").unwrap();
        std::fs::write(
            p.join("basic.ini"),
            "\u{feff}[Output]\nMode=Advanced\n[AdvOut]\nRecEncoder=obs_nvenc_av1_tex\nRecRBTime=180\n[Video]\nOutputCX=2560\nOutputCY=1440\nFPSType=0\nFPSCommon=60\n",
        )
        .unwrap();
        std::fs::write(p.join("recordEncoder.json"), r#"{"rate_control":"CQP","cqp":16,"preset":"p6"}"#).unwrap();
        let s = obs_from(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((s.width, s.height, s.fps), (Some(2560), Some(1440), Some(60.0)));
        assert_eq!(s.codec.as_deref(), Some("AV1 (NVENC)"));
        assert_eq!(s.quality.as_deref(), Some("CQP 16"));
        assert_eq!(s.buffer_secs, Some(180));

        let ours = RecorderSettings { width: Some(1920), height: Some(1080), fps: Some(60.0), ..RecorderSettings::empty(App::Rewind, "") };
        let w = mismatches(&[ours, s]);
        assert!(w.iter().any(|m| m.contains("2560×1440 instead of 1920×1080")), "{w:?}");
    }

    #[test]
    fn names_encoders() {
        assert_eq!(codec_name("jim_hevc_nvenc"), "HEVC (NVENC)");
        assert_eq!(codec_name("obs_x264"), "H.264 (CPU x264)");
        assert_eq!(codec_name("obs_nvenc_h264_tex"), "H.264 (NVENC)");
    }

    /// cargo test read_live_recorder_settings -- --ignored --nocapture
    #[test]
    #[ignore]
    fn read_live_recorder_settings() {
        for a in [App::Obs, App::Nvidia, App::Medal] {
            println!("{:?}", read(a));
        }
    }
}

#[cfg(test)]
mod live_folder_tests {
    /// Prints the clip folders found on this PC (reads settings only).
    #[test]
    #[ignore]
    fn live_clip_folders() {
        for f in super::clip_folders() {
            println!("{} | {} | exists={}", f.label, f.path, f.exists);
        }
    }
}
