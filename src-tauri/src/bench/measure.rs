//! Measuring one stretch of a performance test: the game's frame rate (via
//! PresentMon) plus CPU, RAM, GPU and video-encoder use of each recorder's
//! processes (via Windows' own counters).

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::core::PCWSTR;

/// A recorder, identified by the exe names of its processes. Child processes
/// (helpers, browser panes) are counted with their parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum App {
    Rewind,
    Obs,
    Nvidia,
    Medal,
}

impl App {
    pub const ALL: [App; 4] = [App::Rewind, App::Obs, App::Nvidia, App::Medal];

    fn matches(self, exe: &str) -> bool {
        let e = exe.to_ascii_lowercase();
        match self {
            App::Rewind => e == "hindsight.exe" || e == "rewind.exe",
            App::Obs => e == "obs64.exe" || e == "obs-ffmpeg-mux.exe",
            App::Nvidia => matches!(
                e.as_str(),
                "nvcontainer.exe" | "nvidia share.exe" | "nvidia overlay.exe" | "nvsphelper64.exe" | "nvidia app.exe"
            ),
            App::Medal => e.contains("medal"),
        }
    }
}

struct Proc {
    pid: u32,
    parent: u32,
    exe: String,
}

fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push(Proc { pid: e.th32ProcessID, parent: e.th32ParentProcessID, exe: String::from_utf16_lossy(&e.szExeFile[..len]) });
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Every process belonging to each app (the named ones plus their children).
pub fn app_pids() -> HashMap<App, Vec<u32>> {
    let procs = processes();
    let mut out: HashMap<App, Vec<u32>> = HashMap::new();
    for app in App::ALL {
        let mut set: Vec<u32> = procs.iter().filter(|p| app.matches(&p.exe)).map(|p| p.pid).collect();
        // Pull in descendants (e.g. Hindsight's WebView, OBS's browser sources, ffmpeg helpers).
        loop {
            let before = set.len();
            for p in &procs {
                if !set.contains(&p.pid) && set.contains(&p.parent) && p.pid != 0 {
                    set.push(p.pid);
                }
            }
            if set.len() == before {
                break;
            }
        }
        out.insert(app, set);
    }
    out
}

pub fn exe_of(pid: u32) -> Option<String> {
    processes().into_iter().find(|p| p.pid == pid).map(|p| p.exe)
}

fn ft(f: FILETIME) -> f64 {
    (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 1e7
}

/// (CPU seconds used so far, working set bytes) for a process, if readable.
fn cpu_and_ram(pid: u32) -> Option<(f64, u64)> {
    unsafe {
        let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let times = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).ok().map(|_| ft(k) + ft(u));
        // Private memory (what the process alone owns), so memory shared
        // between an app's helper processes isn't counted several times.
        let mut mem = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let ram = GetProcessMemoryInfo(h, &mut mem as *mut _ as *mut PROCESS_MEMORY_COUNTERS, mem.cb)
            .ok()
            .map(|_| mem.PrivateUsage as u64)
            .unwrap_or(0);
        let _ = CloseHandle(h);
        times.map(|t| (t, ram))
    }
}

/// GPU engine use per process, averaged between two `sample()` calls.
pub struct GpuEngines {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl GpuEngines {
    pub fn start() -> Option<GpuEngines> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            let _ = PdhCollectQueryData(query);
            Some(GpuEngines { query, counter })
        }
    }

    /// Average % per (pid, engine type) since `start()` (or the last call).
    pub fn sample(&self) -> HashMap<(u32, String), f64> {
        let mut out = HashMap::new();
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return out;
            }
            let (mut size, mut count) = (0u32, 0u32);
            let r = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if r != PDH_MORE_DATA || size == 0 {
                return out;
            }
            let mut buf = vec![0u8; size as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return out;
            }
            for i in 0..count as usize {
                let item = &*items.add(i);
                let name = item.szName.to_string().unwrap_or_default();
                // e.g. pid_1234_luid_0x00000000_0x0000D0E2_phys_0_eng_0_engtype_VideoEncode
                let pid = name.split("pid_").nth(1).and_then(|s| s.split('_').next()).and_then(|s| s.parse::<u32>().ok());
                let eng = name.split("engtype_").nth(1).map(str::to_string);
                if let (Some(pid), Some(eng)) = (pid, eng) {
                    *out.entry((pid, eng)).or_insert(0.0) += item.FmtValue.Anonymous.doubleValue;
                }
            }
        }
        out
    }
}

impl Drop for GpuEngines {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

/// How much of the GPU's video encoder an app is using right now (~2 s sample).
pub fn encoder_use(app: App) -> f64 {
    let Some(g) = GpuEngines::start() else { return 0.0 };
    std::thread::sleep(Duration::from_secs(2));
    let s = g.sample();
    let pids = app_pids().remove(&app).unwrap_or_default();
    s.iter().filter(|((pid, eng), _)| pids.contains(pid) && eng.starts_with("VideoEncode")).map(|(_, v)| v).sum()
}

/// Video-encoder use of every recorder at once (one ~2 s sample).
pub fn encoder_use_all() -> HashMap<App, f64> {
    let Some(g) = GpuEngines::start() else { return HashMap::new() };
    std::thread::sleep(Duration::from_secs(2));
    let s = g.sample();
    app_pids()
        .into_iter()
        .map(|(app, pids)| {
            let v = s.iter().filter(|((pid, eng), _)| pids.contains(pid) && eng.starts_with("VideoEncode")).map(|(_, v)| v).sum();
            (app, v)
        })
        .collect()
}

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppUse {
    /// % of the whole CPU (all cores).
    pub cpu_percent: f64,
    pub ram_mb: f64,
    pub gpu_3d_percent: f64,
    pub encoder_percent: f64,
}

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Frames {
    pub frames: usize,
    pub avg_fps: f64,
    /// Average FPS of the slowest 1% of frames.
    pub low1_fps: f64,
    pub avg_frame_ms: f64,
    /// Average time the GPU spent working on each frame (PresentMon's
    /// MsGPUBusy). Still shows a recorder's cost when FPS is capped.
    pub gpu_busy_ms: Option<f64>,
    /// Average time the game's CPU thread spent working on each frame.
    pub cpu_busy_ms: Option<f64>,
    /// Most common present sync interval: 1+ means V-Sync is on.
    pub sync_interval: Option<i32>,
    /// Most common present mode, e.g. "Hardware: Independent Flip".
    pub present_mode: Option<String>,
}

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub seconds: f64,
    pub frames: Option<Frames>,
    pub apps: HashMap<App, AppUse>,
}

fn cores() -> f64 {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64
}

/// Measure for `secs`: the game's frames (if `game_pid`) and every app's use.
#[cfg(test)]
pub fn measure(secs: u32, game_pid: Option<u32>, presentmon: &Path, scratch: &Path) -> Window {
    measure_with(secs, game_pid, presentmon, scratch, || {})
}

/// Like `measure`, calling `tick` about every 250 ms while it runs.
pub fn measure_with(secs: u32, game_pid: Option<u32>, presentmon: &Path, scratch: &Path, mut tick: impl FnMut()) -> Window {
    let pids = app_pids();
    let cpu0: HashMap<u32, f64> = pids.values().flatten().filter_map(|&p| cpu_and_ram(p).map(|(c, _)| (p, c))).collect();
    let gpu = GpuEngines::start();
    let pm = game_pid.and_then(|pid| start_presentmon(presentmon, pid, secs, scratch));
    let t0 = Instant::now();
    let until = t0 + Duration::from_secs(secs as u64);
    while Instant::now() < until {
        tick();
        std::thread::sleep(Duration::from_millis(250).min(until.saturating_duration_since(Instant::now())));
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let engines = gpu.as_ref().map(|g| g.sample()).unwrap_or_default();

    let mut apps = HashMap::new();
    for (app, list) in &pids {
        let mut u = AppUse::default();
        for &p in list {
            if let Some((c1, ram)) = cpu_and_ram(p) {
                if let Some(c0) = cpu0.get(&p) {
                    u.cpu_percent += (c1 - c0) / elapsed / cores() * 100.0;
                }
                u.ram_mb += ram as f64 / 1024.0 / 1024.0;
            }
        }
        for ((pid, eng), v) in &engines {
            if list.contains(pid) {
                if eng.starts_with("3D") {
                    u.gpu_3d_percent += v;
                } else if eng.starts_with("VideoEncode") {
                    u.encoder_percent += v;
                }
            }
        }
        apps.insert(*app, u);
    }
    let frames = pm.and_then(|(child, csv)| finish_presentmon(child, &csv, game_pid.unwrap()));
    Window { seconds: elapsed, frames, apps }
}

fn start_presentmon(exe: &Path, pid: u32, secs: u32, scratch: &Path) -> Option<(Child, PathBuf)> {
    let csv = scratch.join(format!("frames-{pid}-{}.csv", chrono::Local::now().format("%H%M%S%f")));
    let child = Command::new(exe)
        .args(["--process_id", &pid.to_string(), "--timed", &secs.to_string(), "--terminate_after_timed"])
        .args(["--no_console_stats", "--stop_existing_session", "--session_name", "RewindBench", "--output_file"])
        .arg(&csv)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // no console window
        .spawn()
        .ok()?;
    Some((child, csv))
}

fn finish_presentmon(mut child: Child, csv: &Path, pid: u32) -> Option<Frames> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let text = std::fs::read_to_string(csv).ok()?;
    let _ = std::fs::remove_file(csv);
    frames_from_csv(&text, pid)
}

/// Frame stats from PresentMon's CSV (`MsBetweenPresents` of the game's rows).
pub fn frames_from_csv(text: &str, pid: u32) -> Option<Frames> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next()?.split(',').collect();
    let col = |name: &str| header.iter().position(|h| *h == name);
    let (pid_col, ms_col) = (col("ProcessID")?, col("MsBetweenPresents")?);
    let (gpu_col, cpu_col, sync_col, mode_col) = (col("MsGPUBusy"), col("MsCPUBusy"), col("SyncInterval"), col("PresentMode"));
    let mut ms: Vec<f64> = Vec::new();
    let (mut gpu, mut cpu) = (Vec::new(), Vec::new());
    let mut syncs: HashMap<i32, usize> = HashMap::new();
    let mut modes: HashMap<String, usize> = HashMap::new();
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        if f.get(pid_col).and_then(|s| s.parse::<u32>().ok()) != Some(pid) {
            continue;
        }
        let Some(v) = f.get(ms_col).and_then(|s| s.parse::<f64>().ok()).filter(|v| *v > 0.0) else { continue };
        ms.push(v);
        let num = |c: Option<usize>| c.and_then(|c| f.get(c)).and_then(|s| s.parse::<f64>().ok());
        if let Some(g) = num(gpu_col) {
            gpu.push(g);
        }
        if let Some(c) = num(cpu_col) {
            cpu.push(c);
        }
        if let Some(s) = sync_col.and_then(|c| f.get(c)).and_then(|s| s.parse::<i32>().ok()) {
            *syncs.entry(s).or_default() += 1;
        }
        if let Some(m) = mode_col.and_then(|c| f.get(c)) {
            *modes.entry(m.to_string()).or_default() += 1;
        }
    }
    if ms.len() < 10 {
        return None;
    }
    let total: f64 = ms.iter().sum();
    let mean = |v: &[f64]| (v.len() * 2 >= ms.len()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let (gpu_busy_ms, cpu_busy_ms) = (mean(&gpu), mean(&cpu));
    ms.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let worst = (ms.len() / 100).max(1);
    let worst_avg = ms[..worst].iter().sum::<f64>() / worst as f64;
    Some(Frames {
        frames: ms.len(),
        avg_fps: ms.len() as f64 / (total / 1000.0),
        low1_fps: 1000.0 / worst_avg,
        avg_frame_ms: total / ms.len() as f64,
        gpu_busy_ms,
        cpu_busy_ms,
        sync_interval: syncs.into_iter().max_by_key(|(_, n)| *n).map(|(s, _)| s),
        present_mode: modes.into_iter().max_by_key(|(_, n)| *n).map(|(m, _)| m),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stats_from_presentmon_csv() {
        // 99 frames at 10 ms (100 fps) + 1 frame at 50 ms, for pid 42; other pids ignored.
        let mut csv = String::from("Application,ProcessID,MsBetweenPresents\n");
        for _ in 0..99 {
            csv.push_str("game.exe,42,10.0\n");
        }
        csv.push_str("game.exe,42,50.0\nother.exe,7,1.0\n");
        let f = frames_from_csv(&csv, 42).unwrap();
        assert_eq!(f.frames, 100);
        assert!((f.avg_fps - 100.0 / 1.04).abs() < 0.01, "{}", f.avg_fps);
        assert!((f.low1_fps - 20.0).abs() < 0.01, "{}", f.low1_fps);
        assert_eq!(f.gpu_busy_ms, None);
    }

    #[test]
    fn reads_gpu_busy_and_vsync_columns() {
        let mut csv = String::from("Application,ProcessID,SyncInterval,PresentMode,MsBetweenPresents,MsCPUBusy,MsGPUBusy\n");
        for _ in 0..20 {
            csv.push_str("game.exe,42,1,Hardware: Independent Flip,16.6,4.0,6.0\n");
        }
        let f = frames_from_csv(&csv, 42).unwrap();
        assert_eq!(f.gpu_busy_ms, Some(6.0));
        assert_eq!(f.cpu_busy_ms, Some(4.0));
        assert_eq!(f.sync_interval, Some(1));
        assert_eq!(f.present_mode.as_deref(), Some("Hardware: Independent Flip"));
    }

    /// A stand-in game (ffplay test pattern at 60 fps) measured by PresentMon,
    /// then OBS started with its replay buffer, detected, measured and closed.
    /// cargo test bench_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_live_fps_and_obs() {
        crate::ffmpeg::init(None);
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let pm = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../vendor/presentmon/PresentMon.exe"));
        let mut game = std::process::Command::new(crate::ffmpeg::exe("ffplay"))
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=60", "-window_title", "Hindsight test game"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ffplay");
        std::thread::sleep(Duration::from_secs(3));
        let pid = game.id();
        let w = measure(8, Some(pid), &pm, &std::env::temp_dir());
        let f = w.frames.clone().expect("frames from PresentMon");
        println!("stand-in game: {} frames, avg {:.1} fps, 1% low {:.1} fps", f.frames, f.avg_fps, f.low1_fps);
        assert!((50.0..=70.0).contains(&f.avg_fps), "{}", f.avg_fps);

        if Path::new(r"C:\Program Files\obs-studio\bin\64bit\obs64.exe").exists() {
            if !super::super::run::launch_obs() {
                println!("OBS is set to run as administrator: the test would ask the user to start it.");
                let _ = game.kill();
                return;
            }
            let t = Instant::now();
            let mut enc = 0.0;
            while t.elapsed() < Duration::from_secs(40) {
                enc = encoder_use(App::Obs);
                if enc > 0.3 {
                    break;
                }
            }
            println!("OBS encoding detected after {:?} ({enc:.1}% encoder)", t.elapsed());
            assert!(enc > 0.3, "OBS never started encoding");
            std::thread::sleep(Duration::from_secs(5));
            let w = measure(8, Some(pid), &pm, &std::env::temp_dir());
            let o = &w.apps[&App::Obs];
            println!(
                "with OBS: avg {:.1} fps; OBS CPU {:.2}% of whole CPU, RAM {:.0} MB, GPU 3D {:.1}%, encoder {:.1}%",
                w.frames.as_ref().map(|f| f.avg_fps).unwrap_or(0.0), o.cpu_percent, o.ram_mb, o.gpu_3d_percent, o.encoder_percent
            );
            let _ = super::super::run::close_obs();
            std::thread::sleep(Duration::from_secs(2));
            assert!(app_pids()[&App::Obs].is_empty(), "OBS still running");
            println!("OBS closed");
        }
        let _ = game.kill();
    }

    /// cargo test measure_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn measure_live() {
        let w = measure(5, None, Path::new("PresentMon.exe"), &std::env::temp_dir());
        for (app, u) in &w.apps {
            println!(
                "{app:?}: CPU {:.2}% of whole CPU, RAM {:.0} MB, GPU 3D {:.2}%, encoder {:.2}%",
                u.cpu_percent, u.ram_mb, u.gpu_3d_percent, u.encoder_percent
            );
        }
    }

    #[test]
    fn finds_this_process_and_gpu_counters() {
        let me = std::process::id();
        assert!(cpu_and_ram(me).is_some());
        let g = GpuEngines::start().expect("GPU engine counters");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!g.sample().is_empty(), "no GPU engine instances");
    }
}
