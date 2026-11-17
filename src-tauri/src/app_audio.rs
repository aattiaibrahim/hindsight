//! Per-app audio: Discord on its own track, and desktop audio without it.
//!
//! Uses Windows' process loopback (Windows 10 2004 / 11): capture one program's
//! sound (with its child processes), or everything except one program. The
//! game, browser and the rest stay in "Desktop audio"; Discord voice goes to
//! "Discord audio", so either can be muted or turned down in the editor.
//!
//! Discord can start, quit or restart while recording. The capture checks for
//! it every couple of seconds and re-targets itself; while Discord isn't
//! running its track is silent and desktop audio is simply everything (minus
//! Hindsight's own sounds).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{BLOB, CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, implement};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Target {
    /// Everything except Discord
    AllButDiscord,
    /// Only Discord
    Discord,
}

/// Discord's executables (stable, PTB, Canary, and Vesktop).
const DISCORD: [&str; 4] = ["discord.exe", "discordptb.exe", "discordcanary.exe", "vesktop.exe"];

/// Is this process name one of Discord's?
pub fn is_discord(exe: &str) -> bool {
    DISCORD.contains(&exe.to_ascii_lowercase().as_str())
}

/// From (pid, parent pid, exe) rows: the Discord process at the top of its tree.
pub fn discord_root_of(procs: &[(u32, u32, String)]) -> Option<u32> {
    let discord: Vec<&(u32, u32, String)> = procs.iter().filter(|p| is_discord(&p.2)).collect();
    let parent_is_discord = |ppid: u32| discord.iter().any(|p| p.0 == ppid);
    discord.iter().filter(|p| !parent_is_discord(p.1)).map(|p| p.0).min()
}

fn processes() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

pub fn discord_pid() -> Option<u32> {
    discord_root_of(&processes())
}

// Objects made with #[implement] are already agile, as this callback needs.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Done(Mutex<Option<mpsc::Sender<()>>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
    fn ActivateCompleted(&self, _op: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

/// A running process-loopback capture.
struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

fn open(pid: u32, include: bool) -> Result<Stream, String> {
    unsafe {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: if include {
                        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
                    } else {
                        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
                    },
                },
            },
        };
        // PROPVARIANT frees its contents on drop; the blob points at `params`
        // on the stack, so it must never be dropped.
        let mut pv = std::mem::ManuallyDrop::new(PROPVARIANT::default());
        (*pv.Anonymous.Anonymous).vt = VT_BLOB;
        (*pv.Anonymous.Anonymous).Anonymous.blob =
            BLOB { cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32, pBlobData: &params as *const _ as *mut u8 };
        let (tx, rx) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = Done(Mutex::new(Some(tx))).into();
        let op = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&*pv as *const PROPVARIANT), &handler)
            .map_err(|e| format!("per-app audio isn't available ({e})"))?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "per-app audio didn't respond".to_string())?;
        let mut hr = windows::core::HRESULT(0);
        let mut unk = None;
        op.GetActivateResult(&mut hr, &mut unk).map_err(|e| e.to_string())?;
        hr.ok().map_err(|e| format!("per-app audio isn't available ({e})"))?;
        let client: IAudioClient = unk.ok_or("no audio client")?.cast().map_err(|e| e.to_string())?;
        // Process loopback has no mix format of its own: ask for 48 kHz stereo 16-bit.
        let block = CHANNELS * 2;
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: CHANNELS,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * block as u32,
            nBlockAlign: block,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                2_000_000, // 200 ms
                0,
                &fmt,
                None,
            )
            .map_err(|e| e.to_string())?;
        let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
        let stream = Stream { capture: client.GetService().map_err(|e| e.to_string())?, client, event };
        stream.client.SetEventHandle(event).map_err(|e| e.to_string())?;
        stream.client.Start().map_err(|e| e.to_string())?;
        Ok(stream)
    }
}

/// QPC "now" in 100 ns units, the clock packet timestamps use.
fn qpc_now_100ns() -> u64 {
    unsafe {
        let (mut c, mut f) = (0i64, 0i64);
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
        if f <= 0 {
            return 0;
        }
        (c as i128 * 10_000_000 / f as i128) as u64
    }
}

/// Read what's waiting; `emit` gets each packet with the moment it was captured.
fn drain(s: &Stream, emit: &mut dyn FnMut(Instant, Vec<f32>)) -> Result<(), String> {
    unsafe {
        loop {
            let n = s.capture.GetNextPacketSize().map_err(|e| e.to_string())?;
            if n == 0 {
                return Ok(());
            }
            let (mut data, mut frames, mut flags, mut qpc) = (std::ptr::null_mut(), 0u32, 0u32, 0u64);
            s.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc)).map_err(|e| e.to_string())?;
            let count = frames as usize * CHANNELS as usize;
            let samples: Vec<f32> = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                vec![0.0; count]
            } else {
                std::slice::from_raw_parts(data as *const i16, count).iter().map(|&x| x as f32 / 32768.0).collect()
            };
            s.capture.ReleaseBuffer(frames).map_err(|e| e.to_string())?;
            // How long ago the packet was captured; fall back to its length if the stamp looks wrong.
            let now = qpc_now_100ns();
            let age = if qpc > 0 && now >= qpc && now - qpc < 10_000_000 {
                Duration::from_nanos((now - qpc) * 100)
            } else {
                Duration::from_secs_f64(frames as f64 / RATE as f64)
            };
            emit(Instant::now().checked_sub(age).unwrap_or_else(Instant::now), samples);
        }
    }
}

#[cfg(test)]
/// Check that per-app capture works on this PC (opens and closes a stream).
pub fn available() -> bool {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let ok = open(std::process::id(), false).is_ok();
        let _ = tx.send(ok);
        CoUninitialize();
    });
    rx.recv_timeout(Duration::from_secs(6)).unwrap_or(false)
}

/// Capture `target` on this thread until `stop`, re-targeting when Discord
/// starts or quits. `ready` gets the first open's outcome.
pub fn run(target: Target, stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<(), String>>, mut emit: impl FnMut(Instant, Vec<f32>)) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let me = std::process::id();
    // Who to include or exclude: Discord's root, or (no Discord) ourselves.
    let pick = |d: Option<u32>| match target {
        Target::AllButDiscord => Some(d.unwrap_or(me)),
        Target::Discord => d,
    };
    let mut discord = discord_pid();
    let mut stream = match pick(discord).map(|pid| open(pid, target == Target::Discord)) {
        Some(Err(e)) => {
            let _ = ready.send(Err(e));
            unsafe { CoUninitialize() };
            return;
        }
        Some(Ok(s)) => Some(s),
        None => None,
    };
    let _ = ready.send(Ok(()));
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match &stream {
            Some(s) => {
                unsafe { WaitForSingleObject(s.event, 100) };
                if let Err(e) = drain(s, &mut emit) {
                    crate::log!("[rewind] per-app audio ({target:?}): {e}; reopening");
                    stream = None;
                    last_check = Instant::now() - Duration::from_secs(10);
                }
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
        if last_check.elapsed() >= Duration::from_secs(2) {
            last_check = Instant::now();
            let now = discord_pid();
            if now != discord || stream.is_none() {
                discord = now;
                stream = None;
                if let Some(pid) = pick(discord) {
                    match open(pid, target == Target::Discord) {
                        Ok(s) => stream = Some(s),
                        Err(e) => crate::log!("[rewind] per-app audio ({target:?}): {e}"),
                    }
                }
            }
        }
    }
    drop(stream);
    unsafe { CoUninitialize() };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, exe: &str) -> (u32, u32, String) {
        (pid, ppid, exe.into())
    }

    #[test]
    fn finds_the_top_discord_process() {
        let procs = vec![
            p(4, 0, "System"),
            p(900, 500, "explorer.exe"),
            p(1200, 900, "Discord.exe"),
            p(1300, 1200, "Discord.exe"),
            p(1400, 1300, "Discord.exe"),
            p(2000, 900, "VALORANT.exe"),
        ];
        assert_eq!(discord_root_of(&procs), Some(1200));
        assert_eq!(discord_root_of(&procs[..2]), None);
        assert_eq!(discord_root_of(&[p(70, 1, "DiscordCanary.exe")]), Some(70));
        assert!(is_discord("Vesktop.exe") && !is_discord("discord-helper.exe"));
    }

    /// Opens the real per-app capture on this PC (needs Windows 10 2004+).
    #[test]
    fn per_app_capture_opens() {
        // Needs a real sound device (GitHub's test machines skip it).
        if std::env::var_os("HINDSIGHT_NO_MEDIA").is_some() {
            return;
        }
        assert!(available(), "process loopback failed to open");
    }
}

#[cfg(test)]
mod live {
    use super::*;
    /// Listens for 3 s to Discord and to everything else, printing levels. Plays nothing.
    #[test]
    #[ignore]
    fn live_levels() {
        println!("discord pid: {:?}", discord_pid());
        for t in [Target::Discord, Target::AllButDiscord] {
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel();
            let got = Arc::new(Mutex::new((0usize, 0f32, 0usize)));
            let (g, st) = (got.clone(), stop.clone());
            let h = std::thread::spawn(move || run(t, st, tx, move |_, s| {
                let mut g = g.lock().unwrap();
                g.0 += 1;
                g.2 += s.len();
                g.1 = s.iter().fold(g.1, |m, x| m.max(x.abs()));
            }));
            println!("{t:?} ready: {:?}", rx.recv_timeout(Duration::from_secs(6)));
            std::thread::sleep(Duration::from_secs(3));
            stop.store(true, Ordering::Relaxed);
            h.join().unwrap();
            let g = got.lock().unwrap();
            println!("{t:?}: {} packets, {} samples, peak {:.3}", g.0, g.2, g.1);
        }
    }
}
