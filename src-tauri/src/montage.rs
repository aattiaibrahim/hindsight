//! Montages: many clip pieces, one after another, in one video, with an
//! optional song under them.
//!
//! - Every piece is scaled to one size (letterboxed, never stretched) and one
//!   frame rate, so clips from different recorders and resolutions join
//!   cleanly. Each piece's own audio tracks (game, mic, Discord) are mixed.
//! - The song loops if it's shorter than the montage, is cut to its length,
//!   and fades out over the last two seconds. Clip sound and song each get
//!   their own volume.
//! - Encoded as H.264 on the GPU (plays everywhere), saved to
//!   `<clips>/Montages/Montage <date time>.mp4`. Progress comes from ffmpeg's
//!   `-progress` output.

use crate::{ffmpeg, library};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Piece {
    pub path: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Music {
    pub path: String,
    /// Seconds into the song to start from
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub volume_db: f64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Montage {
    pub pieces: Vec<Piece>,
    pub music: Option<Music>,
    /// Volume of the clips' own sound (dB; -60 or less = silent)
    #[serde(default)]
    pub clip_volume_db: f64,
    /// Output height: 720 / 1080 / 1440 / 2160
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_fps")]
    pub fps: u32,
}

fn default_height() -> u32 {
    1080
}
fn default_fps() -> u32 {
    60
}

/// Set while a render runs: `cancel` stops it.
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// The ffmpeg arguments for a montage (without the output path).
pub fn args(m: &Montage, audio_tracks: &[Vec<usize>]) -> Result<(Vec<std::ffi::OsString>, f64), String> {
    if m.pieces.is_empty() {
        return Err("Add some clips to the montage first.".into());
    }
    let h = m.height.clamp(360, 2160) / 2 * 2;
    let w = (h * 16 / 9) / 2 * 2;
    let fps = m.fps.clamp(24, 120);
    let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-progress", "pipe:1", "-nostats"]);
    let mut total = 0.0;
    for p in &m.pieces {
        let len = (p.end - p.start).max(0.1);
        total += len;
        a.extend(ffmpeg::args(&["-ss", &format!("{:.3}", p.start.max(0.0)), "-t", &format!("{len:.3}"), "-i"]));
        a.push(p.path.clone().into());
    }
    let music_input = m.pieces.len();
    if let Some(mu) = &m.music {
        a.extend(ffmpeg::args(&["-stream_loop", "-1", "-ss", &format!("{:.3}", mu.offset.max(0.0)), "-i"]));
        a.push(mu.path.clone().into());
    }
    let mut g = Vec::new();
    let clip_gain = if m.clip_volume_db <= -60.0 { "volume=0".to_string() } else { format!("volume={:.2}dB", m.clip_volume_db) };
    for (i, p) in m.pieces.iter().enumerate() {
        let len = (p.end - p.start).max(0.1);
        g.push(format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black,fps={fps},format=yuv420p,setsar=1,trim=duration={len:.3},setpts=PTS-STARTPTS[v{i}]"
        ));
        // The clip's sound: its tracks mixed (not a track that's already the others
        // mixed, see library::tracks_to_mix), or silence for a clip without any.
        let mix = audio_tracks.get(i).cloned().unwrap_or_default();
        let tail = format!("aresample=48000,aformat=channel_layouts=stereo,{clip_gain},apad,atrim=duration={len:.3},asetpts=PTS-STARTPTS[a{i}]");
        match mix.as_slice() {
            [] => g.push(format!("anullsrc=r=48000:cl=stereo,atrim=duration={len:.3},asetpts=PTS-STARTPTS[a{i}]")),
            [t] => g.push(format!("[{i}:a:{t}]{tail}")),
            ts => {
                let ins: String = ts.iter().map(|t| format!("[{i}:a:{t}]")).collect();
                g.push(format!("{ins}amix=inputs={}:duration=longest:normalize=0,{tail}", ts.len()));
            }
        }
    }
    let pairs: String = (0..m.pieces.len()).map(|i| format!("[v{i}][a{i}]")).collect();
    g.push(format!("{pairs}concat=n={}:v=1:a=1[vout][aclips]", m.pieces.len()));
    if let Some(mu) = &m.music {
        let fade_at = (total - 2.0).max(0.0);
        g.push(format!(
            "[{music_input}:a:0]aresample=48000,aformat=channel_layouts=stereo,volume={:.2}dB,atrim=duration={total:.3},afade=t=out:st={fade_at:.3}:d=2,asetpts=PTS-STARTPTS[song]",
            mu.volume_db
        ));
        g.push("[aclips][song]amix=inputs=2:duration=first:normalize=0[aout]".into());
    } else {
        g.push("[aclips]anull[aout]".into());
    }
    a.extend(ffmpeg::args(&["-filter_complex", &g.join(";"), "-map", "[vout]", "-map", "[aout]"]));
    a.extend(ffmpeg::args(&[
        "-c:v", "h264_nvenc", "-preset", "p5", "-cq", "21", "-b:v", "0", "-g", &(fps * 2).to_string(),
        "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-t", &format!("{total:.3}"),
    ]));
    Ok((a, total))
}

/// Where a new montage goes.
pub fn out_path(clips_dir: &Path) -> PathBuf {
    let dir = clips_dir.join("Montages");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let mut p = dir.join(format!("Montage {stamp}.mp4"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("Montage {stamp} ({n}).mp4"));
        n += 1;
    }
    p
}

/// Render it. `progress` gets 0..1 as it goes.
pub fn render(m: &Montage, out: &Path, mut progress: impl FnMut(f64)) -> Result<String, String> {
    CANCEL.store(false, Ordering::Relaxed);
    let tracks: Vec<Vec<usize>> = m
        .pieces
        .iter()
        .map(|p| library::tracks_to_mix(&p.path, library::probe(&p.path).map(|i| i.audio_tracks).unwrap_or(0) as usize))
        .collect();
    let (mut a, total) = args(m, &tracks)?;
    a.push(out.into());
    let mut child = ffmpeg::cmd("ffmpeg")
        .args(&a)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stderr = child.stderr.take();
    let err_text = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut e) = stderr {
            let _ = std::io::Read::read_to_string(&mut e, &mut s);
        }
        s
    });
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if CANCEL.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                progress((us / 1e6 / total).clamp(0.0, 1.0));
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let errors = err_text.join().unwrap_or_default();
    if CANCEL.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(out);
        return Err("Cancelled.".into());
    }
    if !status.success() {
        let _ = std::fs::remove_file(out);
        return Err(format!("Making the montage failed: {}", errors.lines().last().unwrap_or("ffmpeg error")));
    }
    progress(1.0);
    Ok(out.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dir: &Path, name: &str, size: &str, audio: u32) -> String {
        crate::ffmpeg::init(None);
        std::fs::create_dir_all(dir).unwrap();
        let out = dir.join(name);
        let mut a = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size={size}:rate=30")]);
        for i in 0..audio {
            a.extend(ffmpeg::args(&["-f", "lavfi", "-i", &format!("sine=frequency={}:sample_rate=44100", 300 + i * 200)]));
        }
        a.extend(ffmpeg::args(&["-t", "4", "-map", "0:v"]));
        for i in 0..audio {
            a.extend(ffmpeg::args(&["-map", &format!("{}:a", i + 1)]));
        }
        a.extend(ffmpeg::args(&["-c:v", "mpeg4", "-c:a", "aac"]));
        a.push(out.clone().into());
        ffmpeg::run("ffmpeg", &a).expect("sample");
        out.to_string_lossy().into_owned()
    }

    #[test]
    fn joins_different_sizes_and_tracks_with_a_song() {
        let d = std::env::temp_dir().join(format!("rewind-montage-{}", std::process::id()));
        let a = sample(&d, "a.mkv", "1280x720", 2); // game + mic
        let b = sample(&d, "b.mp4", "800x600", 0); // no sound, 4:3
        let c = sample(&d, "c.mkv", "1920x1080", 1);
        let song = d.join("song.m4a");
        let mut s = ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=660:sample_rate=48000", "-t", "2", "-c:a", "aac"]);
        s.push(song.clone().into());
        ffmpeg::run("ffmpeg", &s).unwrap();
        let m = Montage {
            pieces: vec![
                Piece { path: a, start: 1.0, end: 3.0 },
                Piece { path: b, start: 0.0, end: 1.5 },
                Piece { path: c, start: 2.0, end: 4.0 },
            ],
            music: Some(Music { path: song.to_string_lossy().into_owned(), offset: 0.0, volume_db: -6.0 }),
            clip_volume_db: 0.0,
            height: 720,
            fps: 30,
        };
        let out = d.join("montage.mp4");
        let mut last = 0.0;
        render(&m, &out, |p| last = p).unwrap();
        let p = library::probe(&out.to_string_lossy()).unwrap();
        assert_eq!((p.codec.as_str(), p.width, p.height, p.audio_tracks), ("h264", 1280, 720, 1));
        assert!((p.duration - 5.5).abs() < 0.3, "{}", p.duration);
        assert_eq!(last, 1.0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn empty_montage_is_an_error() {
        let m = Montage { pieces: vec![], music: None, clip_volume_db: 0.0, height: 1080, fps: 60 };
        assert!(args(&m, &[]).is_err());
    }
}
