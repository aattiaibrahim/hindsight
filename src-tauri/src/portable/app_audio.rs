//! Per-app audio (Discord on its own track) is Windows-only for now. Desktop audio
//! falls back to all desktop sound, as on older Windows.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Instant;

pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Target {
    /// Everything except Discord
    AllButDiscord,
    /// Only Discord
    Discord,
}

pub fn discord_pid() -> Option<u32> {
    None
}

pub fn run(_target: Target, _stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<(), String>>, _emit: impl FnMut(Instant, Vec<f32>)) {
    let _ = ready.send(Err("Per-app audio is only on Windows for now.".into()));
}
