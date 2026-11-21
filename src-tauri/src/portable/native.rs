//! The native engine (Windows Graphics Capture + NVENC) is Windows-only. Elsewhere
//! `encoders::is_nvenc()` is false, so the recorder never starts it.
#![allow(dead_code)]

pub mod nvenc {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Codec {
        H264,
        Hevc,
        Av1,
    }

    impl Codec {
        pub fn setting(self) -> &'static str {
            match self {
                Codec::H264 => "h264",
                Codec::Hevc => "hevc",
                Codec::Av1 => "av1",
            }
        }
    }

    /// No NVENC through this path off Windows.
    pub fn supported_codecs() -> Vec<Codec> {
        Vec::new()
    }

    pub fn nvidia_gpu_name() -> String {
        String::new()
    }
}

pub mod pipeline {
    use crate::audio::AudioRing;
    use crate::{monitors, settings::Settings};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub struct VPacket {
        pub data: Arc<Vec<u8>>,
    }

    pub struct VideoRing {
        pub fps: u32,
        pub packets: VecDeque<VPacket>,
    }

    pub struct AudioTrack {
        pub ring: Arc<Mutex<AudioRing>>,
    }

    pub struct Buffers {
        pub key: String,
        pub video: Mutex<VideoRing>,
        pub audio: Vec<AudioTrack>,
    }

    pub struct Pipeline {
        pub buffers: Arc<Buffers>,
        pub labels: Vec<String>,
        pub audio_error: Option<String>,
    }

    const WINDOWS_ONLY: &str = "Native capture is only on Windows.";

    impl Pipeline {
        pub fn start(_s: &Settings, _m: &monitors::Monitor, _w: u32, _h: u32, _key: String) -> Result<Pipeline, String> {
            Err(WINDOWS_ONLY.into())
        }
        pub fn failed(&self) -> Option<String> {
            Some(WINDOWS_ONLY.into())
        }
        pub fn frame_age(&self) -> Duration {
            Duration::ZERO
        }
        pub fn begin_recording(&self, _dir: &Path, _part: u32) -> Result<(), String> {
            Err(WINDOWS_ONLY.into())
        }
        pub fn end_recording(&self) -> Option<(PathBuf, u32)> {
            None
        }
    }

    pub fn save(_runs: &[Arc<Buffers>], _seconds: u32, _out: &Path, _scratch: &Path) -> Result<(), String> {
        Err(WINDOWS_ONLY.into())
    }

    /// Same marker as on Windows, so a half-saved clip from there is still hidden.
    pub const SAVING: &str = ".saving";

    pub fn recover_saves(_clips_dir: &Path) -> Vec<String> {
        Vec::new()
    }

    pub fn mux_recording_parts(_dir: &Path) {}
}
