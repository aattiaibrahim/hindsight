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

/// The detailed log file, when Settings → Detailed log is on (None = off).
static LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// A log file is started over (the old one kept as hindsight.old.log) past this size.
const LOG_MAX: u64 = 8 * 1024 * 1024;

/// Turn the detailed log file on (in `dir`) or off.
pub fn set_log_file(dir: Option<&Path>) {
    let file = dir.map(|d| d.join("hindsight.log"));
    let mut cur = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
    if *cur == file {
        return;
    }
    if let Some(f) = &file {
        let _ = std::fs::create_dir_all(f.parent().unwrap_or(Path::new(".")));
    }
    *cur = file;
    drop(cur);
    if dir.is_some() {
        crate::log!("[rewind] detailed log on (Hindsight {})", env!("CARGO_PKG_VERSION"));
    }
}

/// Where the detailed log is written, if it's on.
pub fn log_file() -> Option<PathBuf> {
    LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn append(line: &str) {
    use std::io::Write;
    let Some(f) = log_file() else { return };
    if std::fs::metadata(&f).map(|m| m.len() > LOG_MAX).unwrap_or(false) {
        let _ = std::fs::rename(&f, f.with_file_name("hindsight.old.log"));
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(out, "{}", anonymize(line));
    }
}

/// Keep a line for diagnostics (with the time), and in the log file when it's on.
pub fn record(line: String) {
    let now = chrono::Local::now();
    append(&format!("{} {line}", now.format("%Y-%m-%d %H:%M:%S%.3f")));
    let stamped = format!("{} {line}", now.format("%H:%M:%S"));
    let mut l = LINES.lock().unwrap_or_else(|e| e.into_inner());
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

#[cfg(windows)]
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
#[cfg(not(windows))]
fn reg(_path: &str, _name: &str) -> Option<String> {
    None
}

#[cfg(windows)]
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
#[cfg(not(windows))]
fn windows_version() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// GPUs with their driver versions (from DXGI).
#[cfg(windows)]
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
#[cfg(not(windows))]
fn gpus() -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
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
#[cfg(not(windows))]
fn free_space(dir: &str) -> String {
    crate::ffmpeg::free_bytes(std::path::Path::new(dir)).map(|b| format!("{:.1} GB free", b as f64 / 1e9)).unwrap_or_else(|| "unknown".into())
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
    keyframe_gap(out, path);
    track_levels(out, path);
}

/// How far apart the keyframes are in the first 30 s. Seeking decodes from the
/// keyframe before the spot, so a clip with one every 10 s scrubs at a few frames a
/// second (Hindsight's own recordings have one a second).
fn keyframe_gap(out: &mut String, path: &str) {
    let mut a = crate::ffmpeg::args(&["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-read_intervals", "%+30", "-show_entries", "frame=pts_time", "-of", "csv=p=0"]);
    a.push(path.into());
    let Ok(text) = crate::ffmpeg::run("ffprobe", &a) else { return };
    let keys: Vec<f64> = text.lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
    let _ = match keys.len() {
        0 => writeln!(out, "Keyframes in the first 30 s: none found"),
        1 => writeln!(out, "Keyframes in the first 30 s: only one (scrubbing will be slow)"),
        n => {
            let gaps: Vec<f64> = keys.windows(2).map(|w| w[1] - w[0]).collect();
            let max = gaps.iter().cloned().fold(0.0, f64::max);
            let avg = gaps.iter().sum::<f64>() / gaps.len() as f64;
            writeln!(out, "Keyframes in the first 30 s: {n}, every {avg:.1} s on average, at most {max:.1} s apart{}", if max > 3.0 { " (scrubbing will be slow)" } else { "" })
        }
    };
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
    let enc = crate::encoders::found();
    let _ = writeln!(out, "Encoder: {} on {} {:?} (auto records {})", enc.kind.label(), enc.gpu, enc.codecs, crate::encoders::effective("auto"));
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
    for l in LINES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let _ = writeln!(out, "{l}");
    }
    // The detailed log reaches back past restarts.
    if let Some(text) = log_file().and_then(|f| std::fs::read_to_string(f).ok()) {
        section(&mut out, "Detailed log (last 1500 lines)");
        let lines: Vec<&str> = text.lines().collect();
        for l in &lines[lines.len().saturating_sub(1500)..] {
            let _ = writeln!(out, "{l}");
        }
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

    // Windows paths (drive letters, backslashes); Linux and macOS: phase 2 of docs/platforms-plan.md.
    #[cfg(windows)]
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

    // Windows paths (drive letters, backslashes); Linux and macOS: phase 2 of docs/platforms-plan.md.
    #[cfg(windows)]
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
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let clip = std::env::var("REWIND_DIAG_CLIP").ok();
        let r = super::report("0.1.25", &[("Settings", super::settings_json(&crate::settings::Settings::default()))], clip.as_deref());
        std::fs::write(std::env::temp_dir().join("rewind-diag-sample.txt"), &r).unwrap();
        println!("{} lines written", r.lines().count());
    }
}
