//! Hindsight's native recording engine: Windows Graphics Capture → GPU colour
//! convert/resize → NVENC, everything on the GPU and stamped on one clock.

pub mod capture;
pub mod gpu;
pub mod nvenc;
pub mod pipeline;
#[rustfmt::skip]
mod nvenc_sys;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;

    fn primary() -> String {
        crate::monitors::list().into_iter().find(|m| m.primary).expect("a monitor").name
    }

    /// Writes a 24-bit BMP (tests only) so the captured image can be looked at.
    fn write_bmp(path: &std::path::Path, w: u32, h: u32, bgra: &[u8]) {
        let row = (w * 3).div_ceil(4) * 4;
        let mut f = Vec::new();
        let size = 54 + row * h;
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&size.to_le_bytes());
        f.extend_from_slice(&[0; 4]);
        f.extend_from_slice(&54u32.to_le_bytes());
        f.extend_from_slice(&40u32.to_le_bytes());
        f.extend_from_slice(&(w as i32).to_le_bytes());
        f.extend_from_slice(&(-(h as i32)).to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&24u16.to_le_bytes());
        f.extend_from_slice(&[0; 24]);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                f.extend_from_slice(&bgra[i..i + 3]);
            }
            f.resize(f.len() + (row - w * 3) as usize, 0);
        }
        std::fs::write(path, f).unwrap();
    }

    /// Full native pipeline: buffer ~16 s, then save the last 12 s (for the
    /// flash + click sync check, which runs alongside).
    /// cargo test native_pipeline -- --ignored --nocapture
    #[test]
    #[ignore]
    fn native_pipeline_saves_synced_clip() {
        crate::ffmpeg::init(None);
        let s = crate::settings::Settings { buffer_seconds: 30, ..Default::default() };
        let m = crate::monitors::list().into_iter().find(|m| m.primary).unwrap();
        let (w, h) = crate::engine::output_size(&m, s.output_height);
        let cpu0 = cpu_time();
        let t = Instant::now();
        let p = pipeline::Pipeline::start(&s, &m, w as u32, h as u32, "test".into()).expect("pipeline");
        println!("pipeline up in {:?}; audio: {:?} {:?}", t.elapsed(), p.labels, p.audio_error);
        std::thread::sleep(Duration::from_secs(16));
        assert!(p.failed().is_none(), "{:?}", p.failed());
        assert!(p.frame_age() < Duration::from_millis(100), "frames stopped");
        let frames = p.buffers.video.lock().unwrap().packets.len();
        let cpu = cpu_time() - cpu0;
        println!("{frames} frames buffered in 16 s; process CPU {:.2} s ({:.1}% of one core)", cpu, cpu / 16.0 * 100.0);
        let out = std::env::temp_dir().join("rewind-native-sync.mkv");
        let t = Instant::now();
        pipeline::save(&[p.buffers.clone()], 12, &out, &std::env::temp_dir()).expect("save");
        println!("saved in {:?} -> {}", t.elapsed(), out.display());
        let info = crate::library::probe(&out.to_string_lossy()).unwrap();
        println!("clip: {}x{} {} {:.2}s, {} audio tracks", info.width, info.height, info.codec, info.duration, info.audio_tracks);
        assert!((12.0..=13.2).contains(&info.duration), "{}", info.duration);
        assert_eq!(info.audio_tracks as usize, p.buffers.audio.len());
    }

    /// A recording area gives clips of just that area.
    /// cargo test --lib native_area -- --ignored --nocapture
    #[test]
    #[ignore]
    fn native_area_records_only_the_area() {
        crate::ffmpeg::init(None);
        let area = crate::settings::Area { x: 0.3, y: 0.1, w: 0.3, h: 0.8 };
        let s = crate::settings::Settings {
            buffer_seconds: 10,
            capture_area: Some(area),
            desktop_audio: false,
            mic_enabled: false,
            ..Default::default()
        };
        let m = crate::monitors::list().into_iter().find(|m| m.primary).unwrap();
        let (w, h) = crate::engine::capture_size(&s, &m);
        let p = pipeline::Pipeline::start(&s, &m, w as u32, h as u32, "area".into()).expect("pipeline");
        std::thread::sleep(Duration::from_secs(5));
        assert!(p.failed().is_none(), "{:?}", p.failed());
        let out = std::env::temp_dir().join("rewind-native-area.mkv");
        pipeline::save(&[p.buffers.clone()], 3, &out, &std::env::temp_dir()).expect("save");
        let info = crate::library::probe(&out.to_string_lossy()).unwrap();
        println!("area clip {}x{} (expected {w}x{h})", info.width, info.height);
        assert_eq!((info.width, info.height), (w as i64, h as i64));
        if std::env::var_os("REWIND_KEEP").is_none() {
            let _ = std::fs::remove_file(out);
        }
    }

    fn cpu_time() -> f64 {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u).unwrap() };
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
        t(k) + t(u)
    }

    /// Capture → GPU convert → NVENC, then FFmpeg reads the result back.
    /// cargo test native_encode -- --ignored --nocapture
    #[test]
    #[ignore]
    fn native_encode_all_codecs_and_sizes() {
        crate::ffmpeg::init(None);
        let (adapter, hmon, name) = gpu::find_output(&primary()).expect("output");
        let g = gpu::Gpu::new(&adapter, name).unwrap();
        let cap = capture::Capture::start(&g, hmon, true).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let (src, w, h) = {
            let l = cap.latest.lock().unwrap();
            (l.texture.clone(), l.width, l.height)
        };
        let dir = std::env::temp_dir().join("rewind-native-encode");
        let _ = std::fs::create_dir_all(&dir);
        for (codec, ow, oh) in [
            (nvenc::Codec::Av1, w, h),
            (nvenc::Codec::Av1, 1280, 720),
            (nvenc::Codec::Av1, 3840, 2160),
            (nvenc::Codec::Hevc, w, h),
            (nvenc::Codec::H264, w, h),
        ] {
            let fps = 60;
            let conv = gpu::Converter::new(&g, w, h, ow, oh, fps, None).expect("converter");
            let mut enc = nvenc::Encoder::new(
                &g,
                nvenc::EncoderConfig { codec, width: ow, height: oh, fps, quality: 24, gop: fps },
            )
            .expect("encoder");
            let mut bytes = Vec::new();
            let mut keys = 0;
            let t = Instant::now();
            for i in 0..120 {
                conv.convert(&src, enc.next_input()).unwrap();
                let p = enc.encode(i == 0).unwrap();
                if p.key {
                    keys += 1;
                }
                bytes.extend_from_slice(&p.data);
            }
            let per_frame = t.elapsed() / 120;
            let raw = dir.join(format!("{codec:?}_{oh}.{}", codec.raw_format()));
            std::fs::write(&raw, &bytes).unwrap();
            let mkv = raw.with_extension("mkv");
            let mut args = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-r", "60", "-f", codec.raw_format(), "-i"]);
            args.push(raw.clone().into());
            args.extend(crate::ffmpeg::args(&["-c", "copy"]));
            args.push(mkv.clone().into());
            crate::ffmpeg::run("ffmpeg", &args).expect("mux");
            let p = crate::library::probe(&mkv.to_string_lossy()).unwrap();
            println!(
                "{codec:?} {ow}x{oh}: {per_frame:?}/frame, {keys} keyframes, {} KB -> probe {}x{} {} {:.2}s",
                bytes.len() / 1024, p.width, p.height, p.codec, p.duration
            );
            assert_eq!((p.width as u32, p.height as u32), (ow, oh));
            assert_eq!(keys, 2, "keyframe every second");
            assert!((p.duration - 2.0).abs() < 0.1);
            if codec == nvenc::Codec::Av1 && ow == w {
                let png = dir.join("frame.png");
                let mut a = crate::ffmpeg::args(&["-hide_banner", "-loglevel", "error", "-y", "-i"]);
                a.push(mkv.clone().into());
                a.extend(crate::ffmpeg::args(&["-frames:v", "1", "-vf", "scale=960:-2"]));
                a.push(png.clone().into());
                crate::ffmpeg::run("ffmpeg", &a).unwrap();
                println!("decoded frame: {}", png.display());
            }
        }
    }

    /// cargo test native_capture -- --ignored --nocapture
    #[test]
    #[ignore]
    fn native_capture_frames_and_converts() {
        let (adapter, hmon, name) = gpu::find_output(&primary()).expect("output");
        let g = gpu::Gpu::new(&adapter, name.clone()).unwrap();
        println!("GPU: {name}");
        let cap = capture::Capture::start(&g, hmon, true).unwrap();
        let start = Instant::now();
        std::thread::sleep(Duration::from_millis(1500));
        let (frames, w, h, tex) = {
            let l = cap.latest.lock().unwrap();
            (l.frames, l.width, l.height, l.texture.clone())
        };
        println!("captured {frames} frames in {:?} at {w}x{h}", start.elapsed());
        assert!(frames > 0, "no frames arrived");

        let (_, _, px) = g.read_bgra(&tex).unwrap();
        let out = std::env::temp_dir().join("rewind-native-capture.bmp");
        write_bmp(&out, w, h, &px);
        println!("frame saved to {}", out.display());

        // GPU convert to NV12 at native, 1280x720 and 3840x2160.
        for (ow, oh) in [(w, h), (1280, 720), (3840, 2160)] {
            let conv = gpu::Converter::new(&g, w, h, ow, oh, 60, None).expect("converter");
            let dst = g.texture(ow, oh, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET).expect("nv12 texture");
            let t = Instant::now();
            for _ in 0..60 {
                conv.convert(&tex, &dst).expect("convert");
            }
            unsafe { g.context.Flush() };
            println!("convert {w}x{h} -> NV12 {ow}x{oh}: 60 frames queued in {:?}", t.elapsed());
        }
    }
}
