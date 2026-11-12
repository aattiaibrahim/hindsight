//! Diagnostics: one text file a beta tester can send when something looks
//! wrong, so the problem can be found without access to their PC.
//!
//! - What's in it: app and Windows versions, GPU and driver, which encoders
//!   the GPU has, screens, audio devices, the recorder's status and recent
//!   messages, settings, NAS and health-check state, free space, and (when a
//!   clip is given) that clip's streams, health and first decode errors.
//! - What's not: secrets (the Discord webhook, API keys) aren't in settings
//!   to begin with; the Streamable email is left out; the Windows user name
//!   is replaced by %USERPROFILE% in every path.
//! - Recent messages come from `log!`, which keeps the last few hundred lines
//!   the app printed (they also still go to the console).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const KEEP: usize = 400;

/// Keep a line for diagnostics (with the time).
pub fn record(line: String) {
    let stamped = format!("{} {line}", chrono::Local::now().format("%H:%M:%S"));
    let mut l = LINES.lock().unwrap();
    l.push_back(stamped);
    while l.len() > KEEP {
        l.pop_front();
    }
}

/// `eprintln!` that is also kept for diagnostics.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {{
        let line = format!($($t)*);
        eprintln!("{line}");
        $crate::diag::record(line);
    }};
}

/// Replace the user's own folder in paths with %USERPROFILE%.
pub fn anonymize(text: &str) -> String {
    let mut out = text.to_string();
    if let Ok(home) = std::env::var("USERPROFILE")
        && home.len() > 3
    {
        out = out.replace(&home, "%USERPROFILE%").replace(&home.replace('\\', "\\\\"), "%USERPROFILE%");
    }
    if let Ok(user) = std::env::var("USERNAME")
        && user.len() > 2
    {
        out = out.replace(&format!("\\{user}\\"), "\\%USERNAME%\\");
    }
    out
}

fn reg(path: &str, name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let mut buf = vec![0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, &HSTRING::from(path), &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
            .ok()
            .ok()?;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
}

fn windows_version() -> String {
    let k = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let mut product = reg(k, "ProductName").unwrap_or_default();
    let build: u32 = reg(k, "CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
    // Windows 11 still says "Windows 10" in ProductName.
    if build >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    format!("{product} {} (build {build})", reg(k, "DisplayVersion").unwrap_or_default())
}

/// GPUs with their driver versions (from DXGI).
fn gpus() -> Vec<String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1};
    use windows::core::Interface;
    let mut out = Vec::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_string();
            let driver = a
                .CheckInterfaceSupport(&IDXGIDevice::IID)
                .map(|v| {
                    let v = v as u64;
                    format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
                })
                .unwrap_or_else(|_| "?".into());
            out.push(format!("{name} (vendor {:04x}, {} MB VRAM, driver {driver})", d.VendorId, d.DedicatedVideoMemory / 1_048_576));
        }
    }
    out
}

fn free_space(dir: &str) -> String {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::HSTRING;
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        if GetDiskFreeSpaceExW(&HSTRING::from(dir), Some(&mut free), Some(&mut total), None).is_ok() {
            return format!("{:.1} GB free of {:.1} GB", free as f64 / 1e9, total as f64 / 1e9);
        }
    }
    "unknown".into()
}

fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n== {title} ==");
}

/// One clip in depth: its streams, the health check's verdict and the first
/// decode errors ffmpeg reports.
fn clip_report(out: &mut String, path: &str) {
    section(out, "Clip");
    let _ = writeln!(out, "File: {path}");
    if let Ok(m) = std::fs::metadata(path) {
        let _ = writeln!(out, "Size: {} bytes", m.len());
    }
    let mut a = crate::ffmpeg::args(&["-v", "error", "-show_format", "-show_streams", "-of", "json", "-i"]);
    a.push(path.into());
    match crate::ffmpeg::run("ffprobe", &a) {
        Ok(j) => {
            let _ = writeln!(out, "Streams:\n{j}");
        }
        Err(e) => {
            let _ = writeln!(out, "ffprobe failed: {e}");
        }
    }
    // The first 20 s decoded on the GPU (like the health check: software
    // decoding quietly hides the damage), errors only.
    let mut a = crate::ffmpeg::args(&["-hide_banner", "-v", "error", "-hwaccel", "d3d11va", "-t", "20", "-i"]);
    a.push(path.into());
    a.extend(crate::ffmpeg::args(&["-map", "0:v:0", "-f", "null", "-"]));
    if let Ok(o) = crate::ffmpeg::cmd("ffmpeg").args(&a).output() {
        let text = String::from_utf8_lossy(&o.stderr);
        let lines: Vec<&str> = text.lines().filter(|l| crate::health::is_decode_error(l)).collect();
        let _ = writeln!(out, "Decode errors in the first 20 s: {}", lines.len());
        for l in lines.iter().take(30) {
            let _ = writeln!(out, "  {l}");
        }
    }
    track_levels(out, path);
}

/// How loud each audio track is over the first minute, so a silent "Desktop
/// audio" (the game's sound missing) shows at a glance.
fn track_levels(out: &mut String, path: &str) {
    let titles = crate::library::track_titles(path);
    if titles.is_empty() {
        return;
    }
    let _ = writeln!(out, "Audio levels in the first 60 s:");
    for (i, title) in titles.iter().enumerate() {
        let mut a = crate::ffmpeg::args(&["-hide_banner", "-nostats", "-t", "60", "-i"]);
        a.push(path.into());
        a.extend(crate::ffmpeg::args(&["-map", &format!("0:a:{i}"), "-af", "volumedetect", "-f", "null", "-"]));
        let Ok(o) = crate::ffmpeg::cmd("ffmpeg").args(&a).output() else { continue };
        let (mean, max) = parse_volumedetect(&String::from_utf8_lossy(&o.stderr));
        let name = if title.is_empty() { format!("Track {}", i + 1) } else { title.clone() };
        let _ = match (mean, max) {
            (Some(mean), Some(max)) => writeln!(out, "  {name}: average {mean:.1} dB, loudest {max:.1} dB{}", if max < -60.0 { " (silent)" } else { "" }),
            _ => writeln!(out, "  {name}: couldn't measure"),
        };
    }
}

/// ffmpeg's volumedetect summary: (mean_volume, max_volume) in dB.
fn parse_volumedetect(text: &str) -> (Option<f64>, Option<f64>) {
    let find = |key: &str| {
        text.lines().find_map(|l| {
            let rest = &l[l.find(key)? + key.len()..];
            rest.trim().trim_end_matches("dB").trim().parse::<f64>().ok()
        })
    };
    (find("mean_volume:"), find("max_volume:"))
}

/// Everything above, as text. `extra` is JSON the app adds (status, settings…).
pub fn report(version: &str, extra: &[(&str, String)], clip: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Hindsight diagnostics");
    let _ = writeln!(out, "Version: {version} (beta)");
    let _ = writeln!(out, "Made: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z"));
    section(&mut out, "PC");
    let _ = writeln!(out, "Windows: {}", windows_version());
    let _ = writeln!(out, "CPU: {}", reg(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").unwrap_or_default().trim());
    for g in gpus() {
        let _ = writeln!(out, "GPU: {g}");
    }
    let enc = crate::native::nvenc::supported_codecs();
    let _ = writeln!(out, "NVENC: {:?} (auto records {})", enc, crate::native::nvenc::effective_encoder("auto"));
    for m in crate::monitors::list() {
        let _ = writeln!(out, "Screen: {} {}x{} at {},{}{}", m.name, m.width, m.height, m.x, m.y, if m.primary { " (primary)" } else { "" });
    }
    section(&mut out, "Audio");
    let (speakers, mic) = crate::audio::default_devices();
    let _ = writeln!(out, "Default output: {}", speakers.unwrap_or_else(|| "none".into()));
    let _ = writeln!(out, "Default input: {}", mic.unwrap_or_else(|| "none".into()));
    let _ = writeln!(out, "Inputs: {}", crate::audio::input_devices().join(" | "));
    let _ = writeln!(out, "Discord running: {}", crate::app_audio::discord_pid().is_some());
    // Which programs play to which output: a game on a headset that isn't the
    // default output is the usual reason its sound is missing from clips.
    for l in crate::audio_sessions::report_lines(&crate::audio_sessions::outputs()) {
        let _ = writeln!(out, "{l}");
    }
    for (title, body) in extra {
        section(&mut out, title);
        let _ = writeln!(out, "{body}");
    }
    if let Some(p) = clip {
        clip_report(&mut out, p);
    }
    section(&mut out, "Recent messages");
    for l in LINES.lock().unwrap().iter() {
        let _ = writeln!(out, "{l}");
    }
    anonymize(&out)
}

/// Settings as JSON without the few personal fields.
pub fn settings_json(s: &crate::settings::Settings) -> String {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        for k in ["streamableEmail", "discordWebhookName"] {
            if o.contains_key(k) {
                o.insert(k.into(), serde_json::Value::String("(left out)".into()));
            }
        }
    }
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

pub fn free_space_line(dir: &str) -> String {
    free_space(dir)
}

/// Save the report in Downloads (or the temp folder) and return where.
pub fn save(text: &str) -> Result<PathBuf, String> {
    let dir = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")).filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("Hindsight diagnostics {}.txt", chrono::Local::now().format("%Y-%m-%d %H-%M-%S")));
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

#[allow(dead_code)]
fn _exists(p: &Path) -> bool {
    p.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_folder_is_hidden_and_messages_are_kept() {
        let home = std::env::var("USERPROFILE").unwrap();
        assert_eq!(anonymize(&format!("{home}\\Videos\\a.mkv")), "%USERPROFILE%\\Videos\\a.mkv");
        let escaped = format!("\"{}\\\\Videos\"", home.replace('\\', "\\\\"));
        assert_eq!(anonymize(&escaped), "\"%USERPROFILE%\\\\Videos\"");
        crate::log!("[rewind] test line {}", 42);
        assert!(LINES.lock().unwrap().iter().any(|l| l.ends_with("[rewind] test line 42")));
    }

    #[test]
    fn volumedetect_is_read() {
        let t = "[Parsed_volumedetect_0 @ 0x1] n_samples: 960000\n[Parsed_volumedetect_0 @ 0x1] mean_volume: -91.0 dB\n[Parsed_volumedetect_0 @ 0x1] max_volume: -91.0 dB";
        assert_eq!(parse_volumedetect(t), (Some(-91.0), Some(-91.0)));
        assert_eq!(parse_volumedetect("nothing"), (None, None));
    }

    #[test]
    fn report_has_the_pc_and_hides_the_user() {
        let r = report("0.0.0", &[("Settings", settings_json(&crate::settings::Settings::default()))], None);
        assert!(r.contains("Windows:") && r.contains("GPU:") && r.contains("== Settings =="), "{r}");
        assert!(!r.contains(&std::env::var("USERPROFILE").unwrap()));
    }
}

#[cfg(test)]
mod live {
    /// cargo test live_diagnostics -- --ignored --nocapture (REWIND_DIAG_CLIP=path for a clip)
    #[test]
    #[ignore]
    fn live_diagnostics() {
        crate::ffmpeg::init(None);
        let clip = std::env::var("REWIND_DIAG_CLIP").ok();
        let r = super::report("0.1.25", &[("Settings", super::settings_json(&crate::settings::Settings::default()))], clip.as_deref());
        std::fs::write(std::env::temp_dir().join("rewind-diag-sample.txt"), &r).unwrap();
        println!("{} lines written", r.lines().count());
    }
}
