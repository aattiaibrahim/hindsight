//! Which video encoder this PC has, and how to drive it from FFmpeg.
//!
//! NVIDIA uses NVENC (and the native capture engine). Every other PC goes through
//! FFmpeg with whatever the hardware offers:
//!   - AMD: AMF (h264_amf, hevc_amf, av1_amf)
//!   - Intel: Quick Sync (h264_qsv, hevc_qsv, av1_qsv)
//!   - anything else: Windows' own Media Foundation encoders (h264_mf, …), which
//!     use the GPU's encoder when Windows has a driver for it and Microsoft's
//!     software one otherwise
//!   - last resort (Windows "N" editions without Media Foundation): OpenH264 on the CPU
//!
//! Each candidate is tried once with a tiny test encode, so a driver that's
//! missing or too old is skipped instead of breaking recording.
//! `HINDSIGHT_ENCODER=amf|qsv|mf|openh264|nvenc` forces one (for testing).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Media Foundation: the GPU's own encoder works (else Microsoft's software one,
/// which is slower and keeps to a bitrate even less).
static MF_HW: AtomicBool = AtomicBool::new(false);
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Nvenc,
    Amf,
    Qsv,
    Mf,
    OpenH264,
}

impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Kind::Nvenc => "nvenc",
            Kind::Amf => "amf",
            Kind::Qsv => "qsv",
            Kind::Mf => "mf",
            Kind::OpenH264 => "openh264",
        }
    }

    /// In words, for Settings ("on your GPU (AMD AMF)").
    pub fn label(self) -> &'static str {
        match self {
            Kind::Nvenc => "NVIDIA NVENC",
            Kind::Amf => "AMD AMF",
            Kind::Qsv => "Intel Quick Sync",
            Kind::Mf => "Windows Media Foundation",
            Kind::OpenH264 => "OpenH264 (CPU)",
        }
    }

    fn from_name(s: &str) -> Option<Kind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "nvenc" | "nvidia" => Kind::Nvenc,
            "amf" | "amd" => Kind::Amf,
            "qsv" | "intel" => Kind::Qsv,
            "mf" | "windows" => Kind::Mf,
            "openh264" | "cpu" => Kind::OpenH264,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Found {
    pub kind: Kind,
    /// The graphics card's name ("" if unknown)
    pub gpu: String,
    /// "av1" | "hevc" | "h264" it can encode, best first
    pub codecs: Vec<&'static str>,
}

static FOUND: Mutex<Option<Found>> = Mutex::new(None);

/// This PC's encoder, worked out once (a few test encodes the first time).
pub fn found() -> Found {
    if let Some(f) = FOUND.lock().unwrap().clone() {
        return f;
    }
    let f = detect();
    // Without ffmpeg (not unpacked yet) the test encodes all fail: don't keep that.
    if crate::ffmpeg::exe("ffmpeg").is_file() || f.kind == Kind::Nvenc {
        crate::log!("[rewind] video encoder: {} ({}) on {}", f.kind.label(), f.codecs.join(", "), if f.gpu.is_empty() { "unknown GPU" } else { &f.gpu });
        *FOUND.lock().unwrap() = Some(f.clone());
    }
    f
}

pub fn kind() -> Kind {
    found().kind
}

pub fn is_nvenc() -> bool {
    kind() == Kind::Nvenc
}

/// The FFmpeg encoder for a codec ("hevc" → "hevc_amf").
pub fn name(kind: Kind, codec: &str) -> String {
    match kind {
        Kind::OpenH264 => "libopenh264".into(),
        k => format!("{}_{}", codec_or_h264(codec), k.suffix()),
    }
}

fn codec_or_h264(codec: &str) -> &'static str {
    match codec {
        "av1" => "av1",
        "hevc" | "h265" => "hevc",
        _ => "h264",
    }
}

/// The codec to really use for a setting: "auto" is the best this PC has (AV1,
/// then HEVC, then H.264); a choice it can't do falls back the same way.
pub fn effective(setting: &str) -> &'static str {
    pick(setting, &found().codecs)
}

pub fn pick(setting: &str, supported: &[&'static str]) -> &'static str {
    if supported.is_empty() {
        return if setting == "auto" { "h264" } else { codec_or_h264(setting) };
    }
    if setting != "auto" {
        let want = codec_or_h264(setting);
        if supported.contains(&want) {
            return want;
        }
    }
    ["av1", "hevc", "h264"].into_iter().find(|c| supported.contains(c)).unwrap_or("h264")
}

/// The codec an edit of a `codec` file is made in here: the same one if this PC
/// can encode it, else H.264.
pub fn codec_for(codec: &str) -> &'static str {
    let f = found();
    let c = codec_or_h264(codec);
    if f.kind == Kind::Nvenc || f.codecs.contains(&c) { c } else { "h264" }
}

/// How hard to try: Live (recording, every frame on time), Fast (previews), Good (saved edits).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speed {
    Live,
    Fast,
    Good,
}

#[derive(Clone, Copy, Debug)]
pub enum Rate {
    /// Constant quality on NVENC's -cq scale (0–51, lower is better; 16–28 in use)
    Quality(u32),
    /// Average and peak bitrate (kbit/s), for a target size
    Bitrate { kbps: u64, max_kbps: u64, buf_kbps: u64 },
}

/// Everything after the input for the video: `-c:v <encoder>` and its settings.
/// On NVENC it's exactly `nvenc` (each caller's own tuned options, unchanged);
/// on everything else it's built from `rate` and `speed`. Includes `-pix_fmt`
/// on non-NVIDIA encoders (callers leave theirs in `nvenc`).
pub fn args(codec: &str, rate: Rate, speed: Speed, nvenc: &[&str]) -> Vec<String> {
    let f = found();
    if f.kind == Kind::Nvenc {
        let mut a = vec!["-c:v".to_string(), name(Kind::Nvenc, codec)];
        a.extend(nvenc.iter().map(|s| s.to_string()));
        return a;
    }
    let codec = if f.codecs.contains(&codec_or_h264(codec)) { codec_or_h264(codec) } else { "h264" };
    video_args(f.kind, codec, rate, speed)
}

/// The settings for one encoder (not NVENC: its callers keep their own).
pub fn video_args(kind: Kind, codec: &str, rate: Rate, speed: Speed) -> Vec<String> {
    let codec = codec_or_h264(codec);
    let mut a: Vec<String> = vec!["-c:v".into(), name(kind, codec)];
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match kind {
        Kind::Nvenc => {
            let preset = match speed {
                Speed::Live => "p4",
                Speed::Fast => "p2",
                Speed::Good => "p5",
            };
            push(&["-preset", preset]);
            match rate {
                Rate::Quality(q) => push(&["-rc", "vbr", "-cq", &q.to_string(), "-b:v", "0"]),
                Rate::Bitrate { kbps, max_kbps, buf_kbps } => {
                    push(&["-rc", "vbr", "-b:v", &format!("{kbps}k"), "-maxrate", &format!("{max_kbps}k"), "-bufsize", &format!("{buf_kbps}k")])
                }
            }
            push(&["-pix_fmt", "yuv420p"]);
        }
        Kind::Amf => {
            let quality = match speed {
                Speed::Live | Speed::Fast => "speed",
                Speed::Good => "balanced",
            };
            push(&["-usage", "transcoding", "-quality", quality]);
            match rate {
                Rate::Quality(q) => {
                    // AV1's scale is 0–255 (about 5× H.264's).
                    let q = if codec == "av1" { (q * 5).min(255) } else { q.min(51) };
                    let q = q.to_string();
                    push(&["-rc", "cqp", "-qp_i", &q, "-qp_p", &q]);
                    // Only h264_amf has B-frame QP (hevc_amf has no B-frames setting).
                    if codec == "h264" {
                        push(&["-qp_b", &q]);
                    }
                }
                Rate::Bitrate { kbps, max_kbps, buf_kbps } => {
                    push(&["-rc", "vbr_peak", "-b:v", &format!("{kbps}k"), "-maxrate", &format!("{max_kbps}k"), "-bufsize", &format!("{buf_kbps}k")])
                }
            }
            push(&["-pix_fmt", "nv12"]);
        }
        Kind::Qsv => {
            let preset = match speed {
                Speed::Live | Speed::Fast => "veryfast",
                Speed::Good => "medium",
            };
            push(&["-preset", preset]);
            match rate {
                Rate::Quality(q) => {
                    let q = if codec == "av1" { (q * 5).clamp(1, 255) } else { q.clamp(1, 51) };
                    push(&["-global_quality", &q.to_string()]);
                }
                Rate::Bitrate { kbps, max_kbps, buf_kbps } => {
                    push(&["-b:v", &format!("{kbps}k"), "-maxrate", &format!("{max_kbps}k"), "-bufsize", &format!("{buf_kbps}k")])
                }
            }
            push(&["-pix_fmt", "nv12"]);
        }
        Kind::Mf => {
            if MF_HW.load(Ordering::Relaxed) {
                push(&["-hw_encoding", "1"]);
            }
            if codec == "h264" {
                // High profile (100): Windows' default is Baseline, which looks worse for the size.
                push(&["-profile:v", "100"]);
            }
            if speed == Speed::Live {
                push(&["-scenario", "display_remoting"]);
            }
            match rate {
                // MF's quality is 0–100, higher is better: cq 16 ≈ 88, 20 ≈ 80, 28 ≈ 64.
                Rate::Quality(q) => push(&["-rate_control", "quality", "-quality", &mf_quality(q).to_string()]),
                Rate::Bitrate { kbps, .. } => push(&["-rate_control", "cbr", "-b:v", &format!("{kbps}k")]),
            }
            push(&["-pix_fmt", "nv12"]);
        }
        Kind::OpenH264 => {
            let kbps = match rate {
                // No constant-quality mode: a bitrate that looks about the same at 1080p.
                Rate::Quality(q) => {
                    if q <= 18 {
                        16_000
                    } else if q <= 23 {
                        10_000
                    } else {
                        6_000
                    }
                }
                Rate::Bitrate { kbps, .. } => kbps,
            };
            push(&["-rc_mode", "bitrate", "-b:v", &format!("{kbps}k"), "-allow_skip_frames", "0", "-pix_fmt", "yuv420p"]);
        }
    }
    a
}

fn mf_quality(q: u32) -> u32 {
    (120i64 - 2 * q as i64).clamp(10, 100) as u32
}

/// The graphics cards Windows knows, as (vendor id, name), skipping its basic display driver.
fn adapters() -> Vec<(u32, String)> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.VendorId == 0x1414 {
                continue; // Microsoft Basic Render Driver
            }
            out.push((desc.VendorId, String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string()));
        }
    }
    out
}

const AMD: u32 = 0x1002;
const INTEL: u32 = 0x8086;

/// Which encoders to try, best first, for these cards.
fn candidates(gpus: &[(u32, String)]) -> Vec<Kind> {
    let mut out = Vec::new();
    // A gaming PC's discrete AMD card before an Intel CPU's built-in graphics.
    if gpus.iter().any(|(v, _)| *v == AMD) {
        out.push(Kind::Amf);
    }
    if gpus.iter().any(|(v, _)| *v == INTEL) {
        out.push(Kind::Qsv);
    }
    out.extend([Kind::Mf, Kind::OpenH264]);
    out
}

fn gpu_for(kind: Kind, gpus: &[(u32, String)]) -> String {
    let vendor = match kind {
        Kind::Amf => Some(AMD),
        Kind::Qsv => Some(INTEL),
        _ => None,
    };
    vendor
        .and_then(|v| gpus.iter().find(|(id, _)| *id == v))
        .or_else(|| gpus.first())
        .map(|(_, n)| n.clone())
        .unwrap_or_default()
}

fn detect() -> Found {
    let forced = std::env::var("HINDSIGHT_ENCODER").ok().and_then(|s| Kind::from_name(&s));
    if forced.is_none() || forced == Some(Kind::Nvenc) {
        let codecs = crate::native::nvenc::supported_codecs();
        if !codecs.is_empty() {
            return Found { kind: Kind::Nvenc, gpu: crate::native::nvenc::nvidia_gpu_name(), codecs: codecs.into_iter().map(|c| c.setting()).collect() };
        }
    }
    let gpus = adapters();
    let order = match forced {
        Some(k) if k != Kind::Nvenc => vec![k],
        _ => candidates(&gpus),
    };
    for kind in order {
        if kind == Kind::Mf {
            // The GPU's own encoder through Windows, if it has one.
            MF_HW.store(true, Ordering::Relaxed);
            if !works(kind, "h264") {
                MF_HW.store(false, Ordering::Relaxed);
            }
        }
        let codecs: Vec<&'static str> = if kind == Kind::OpenH264 {
            if works(kind, "h264") { vec!["h264"] } else { vec![] }
        } else {
            ["av1", "hevc", "h264"].into_iter().filter(|c| works(kind, c)).collect()
        };
        if codecs.contains(&"h264") || !codecs.is_empty() {
            return Found { kind, gpu: gpu_for(kind, &gpus), codecs };
        }
        crate::log!("[rewind] {} can't encode here; trying the next encoder", kind.label());
    }
    // Nothing passed (ffmpeg missing?): Media Foundation H.264 is on every normal Windows.
    Found { kind: Kind::Mf, gpu: gpus.first().map(|g| g.1.clone()).unwrap_or_default(), codecs: vec!["h264"] }
}

/// A short test encode: does this encoder start and produce frames on this PC?
fn works(kind: Kind, codec: &str) -> bool {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30", "-frames:v", "10"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(video_args(kind, codec, Rate::Quality(23), Speed::Live));
    a.extend(["-f", "null", "-"].iter().map(|s| s.to_string()));
    crate::ffmpeg::run_limited("ffmpeg", &a, Some(Duration::from_secs(20))).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_best_codec_this_pc_has() {
        assert_eq!(pick("auto", &["av1", "hevc", "h264"]), "av1");
        assert_eq!(pick("auto", &["hevc", "h264"]), "hevc");
        assert_eq!(pick("av1", &["h264"]), "h264");
        assert_eq!(pick("h264", &["av1", "hevc", "h264"]), "h264");
        assert_eq!(pick("auto", &[]), "h264");
    }

    #[test]
    fn amd_before_intel_then_windows() {
        let both = [(INTEL, "Intel UHD 770".to_string()), (AMD, "AMD Radeon RX 7800 XT".to_string())];
        assert_eq!(candidates(&both), vec![Kind::Amf, Kind::Qsv, Kind::Mf, Kind::OpenH264]);
        assert_eq!(gpu_for(Kind::Amf, &both), "AMD Radeon RX 7800 XT");
        assert_eq!(gpu_for(Kind::Qsv, &both), "Intel UHD 770");
        assert_eq!(candidates(&[]), vec![Kind::Mf, Kind::OpenH264]);
    }

    #[test]
    fn encoder_names_and_settings() {
        assert_eq!(name(Kind::Amf, "hevc"), "hevc_amf");
        assert_eq!(name(Kind::Qsv, "av1"), "av1_qsv");
        assert_eq!(name(Kind::Mf, "x"), "h264_mf");
        assert_eq!(name(Kind::OpenH264, "hevc"), "libopenh264");
        let amf = video_args(Kind::Amf, "av1", Rate::Quality(20), Speed::Good).join(" ");
        assert!(amf.contains("-qp_i 100") && !amf.contains("qp_b"), "{amf}");
        let qsv = video_args(Kind::Qsv, "h264", Rate::Bitrate { kbps: 4000, max_kbps: 4000, buf_kbps: 8000 }, Speed::Good).join(" ");
        assert!(qsv.contains("-b:v 4000k -maxrate 4000k") && qsv.ends_with("-pix_fmt nv12"), "{qsv}");
        assert_eq!(mf_quality(20), 80);
        assert_eq!(mf_quality(16), 88);
    }

    /// On a real PC: every encoder's settings are accepted by the bundled FFmpeg
    /// (AMF and Quick Sync can only open on their own hardware; on other PCs they
    /// must fail at opening the device, never on a wrong option).
    /// cargo test --lib encoders_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn encoders_live() {
        crate::ffmpeg::init(None);
        for kind in [Kind::Amf, Kind::Qsv, Kind::Mf, Kind::OpenH264] {
            for codec in ["h264", "hevc", "av1"] {
                if kind == Kind::OpenH264 && codec != "h264" {
                    continue;
                }
                for (rate, speed) in [
                    (Rate::Quality(20), Speed::Good),
                    (Rate::Quality(24), Speed::Live),
                    (Rate::Bitrate { kbps: 3000, max_kbps: 3000, buf_kbps: 6000 }, Speed::Good),
                ] {
                    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "warning", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30", "-frames:v", "10"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    let v = video_args(kind, codec, rate, speed);
                    // Every private option must be this encoder's own (another encoder's is silently ignored).
                    let help = crate::ffmpeg::run("ffmpeg", &["-hide_banner", "-h", &format!("encoder={}", v[1])]).unwrap_or_default();
                    for o in v.iter().filter(|o| o.starts_with('-')) {
                        let generic = ["-c:v", "-profile:v", "-pix_fmt", "-b:v", "-maxrate", "-bufsize", "-g", "-global_quality"].contains(&o.as_str());
                        assert!(generic || help.contains(&format!("\n  {o} ")), "{} has no {o}", v[1]);
                    }
                    a.extend(v);
                    a.extend(["-f", "null", "-"].iter().map(|s| s.to_string()));
                    let r = crate::ffmpeg::run_limited("ffmpeg", &a, Some(Duration::from_secs(30)));
                    let err = r.as_ref().err().cloned().unwrap_or_default();
                    println!("{:?} {codec} {:?}/{:?}: {}", kind, rate, speed, if r.is_ok() { "OK".to_string() } else { err.lines().last().unwrap_or("").to_string() });
                    let bad_option = ["Unrecognized option", "Option not found", "Error setting option", "Invalid argument for", "Undefined constant", "has not been used"];
                    assert!(!bad_option.iter().any(|b| err.contains(b)), "{kind:?} {codec}: {err}");
                }
            }
        }
    }
}
