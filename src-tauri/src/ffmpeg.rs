//! Finding and running the bundled ffmpeg / ffprobe.

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::core::PCWSTR;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Games always win CPU time over capture and exports.
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

static DIR: OnceLock<PathBuf> = OnceLock::new();
static JOB: OnceLock<usize> = OnceLock::new();

/// Use the copy bundled next to the app, or the one in vendor/ during development.
pub fn init(resource_dir: Option<PathBuf>) {
    // Development fallback only: release builds don't carry this folder's path.
    #[cfg(debug_assertions)]
    let dev = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../vendor/ffmpeg/bin"));
    #[cfg(not(debug_assertions))]
    let dev = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("ffmpeg"))).unwrap_or_default();
    let dir = match resource_dir.map(|r| r.join("ffmpeg")) {
        Some(d) if d.join("ffmpeg.exe").exists() => d,
        _ => dev,
    };
    let _ = DIR.set(dir);
}

pub fn exe(name: &str) -> PathBuf {
    DIR.get().cloned().unwrap_or_default().join(format!("{name}.exe"))
}

/// A command with no console window, at below-normal CPU priority.
pub fn cmd(name: &str) -> Command {
    let mut c = Command::new(exe(name));
    c.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    c
}

/// Free space on the drive holding `path`, in bytes.
pub fn free_bytes(path: &std::path::Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::HSTRING;
    // Walk up to a folder that exists (the clips folder may not be created yet).
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(p.as_os_str()), Some(&mut free), None, None).ok()? };
    Some(free)
}

/// Turn string literals into an argument list that can also hold paths.
pub fn args(a: &[&str]) -> Vec<std::ffi::OsString> {
    a.iter().map(|s| s.into()).collect()
}

/// ffmpeg's default AAC search ("twoloop") can loop forever on some audio
/// (seen with a 44.1 kHz mic resampled to 48 kHz, build N-128198). The
/// "fast" coder never does and sounds the same at Hindsight's bitrates, so
/// every AAC encode uses it.
pub const AAC_SAFE: [&str; 2] = ["-aac_coder", "fast"];

/// Add `-aac_coder fast` after every `-c:a aac` / `-c:a:N aac` / `-acodec aac`.
pub fn safe_aac<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Vec<std::ffi::OsString> {
    let mut out = Vec::with_capacity(args.len() + 2);
    let mut prev_was_acodec = false;
    for a in args {
        let a = a.as_ref();
        out.push(a.to_os_string());
        let s = a.to_string_lossy();
        if prev_was_acodec && s == "aac" {
            out.extend(AAC_SAFE.iter().map(|x| x.into()));
        }
        prev_was_acodec = s == "-acodec" || s == "-c:a" || s.starts_with("-c:a:") || s == "-codec:a";
    }
    out
}

/// Run to completion; Ok(stdout) on success, Err(last stderr lines) on failure.
pub fn run<S: AsRef<std::ffi::OsStr>>(name: &str, args: &[S]) -> Result<String, String> {
    run_limited(name, args, None)
}

thread_local! {
    /// Set by `with_progress`: told how many seconds of output each ffmpeg run on
    /// this thread has written so far.
    static PROGRESS: std::cell::RefCell<Option<Box<dyn Fn(f64)>>> = const { std::cell::RefCell::new(None) };
}

/// Run `f`, telling `report` how far each ffmpeg it runs (on this thread) has got,
/// in seconds of output, a few times a second. For edits' progress bars.
pub fn with_progress<T>(report: impl Fn(f64) + 'static, f: impl FnOnce() -> T) -> T {
    PROGRESS.with(|p| *p.borrow_mut() = Some(Box::new(report)));
    let out = f();
    PROGRESS.with(|p| *p.borrow_mut() = None);
    out
}

/// The last `out_time_us` in an ffmpeg `-progress` file, in seconds.
fn progress_secs(file: &std::path::Path) -> Option<f64> {
    let text = std::fs::read_to_string(file).ok()?;
    let us: i64 = text.lines().rev().find_map(|l| l.strip_prefix("out_time_us="))?.trim().parse().ok()?;
    (us >= 0).then(|| us as f64 / 1_000_000.0)
}

/// Like `run`, but kill ffmpeg and fail if it takes longer than `limit`.
pub fn run_limited<S: AsRef<std::ffi::OsStr>>(name: &str, args: &[S], limit: Option<Duration>) -> Result<String, String> {
    let tracked = name == "ffmpeg" && PROGRESS.with(|p| p.borrow().is_some());
    // ffmpeg writes its progress to a small file (its stdout is sometimes the output).
    let progress_file = tracked.then(|| {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("hindsight-progress-{}-{n}.txt", std::process::id()))
    });
    let mut all: Vec<std::ffi::OsString> = Vec::new();
    if let Some(f) = &progress_file {
        all.extend(["-progress".into(), f.as_os_str().to_os_string(), "-stats_period".into(), "0.25".into()]);
    }
    all.extend(safe_aac(args));
    let mut child = cmd(name)
        .args(all)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start {name}: {e}"))?;
    // Read both pipes on their own threads so a chatty ffmpeg can't block.
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let mut last_report = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if limit.is_some_and(|l| started.elapsed() > l) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(f) = &progress_file {
                let _ = std::fs::remove_file(f);
            }
            return Err(format!("{name} got stuck and was stopped after {} s.", started.elapsed().as_secs()));
        }
        if let Some(f) = &progress_file
            && last_report.elapsed() >= Duration::from_millis(250)
        {
            last_report = Instant::now();
            if let Some(secs) = progress_secs(f) {
                PROGRESS.with(|p| {
                    if let Some(report) = p.borrow().as_ref() {
                        report(secs);
                    }
                });
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if let Some(f) = &progress_file {
        let _ = std::fs::remove_file(f);
    }
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&stderr);
        let tail: Vec<&str> = err.lines().rev().take(4).collect();
        Err(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
    }
}

/// Put a child in a job object so it dies with Hindsight, even if Hindsight crashes.
pub fn adopt(child: &Child) {
    let raw = *JOB.get_or_init(|| unsafe {
        let Ok(job) = CreateJobObjectW(None, PCWSTR::null()) else {
            return 0;
        };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        job.0 as usize
    });
    if raw != 0 {
        unsafe {
            let _ = AssignProcessToJobObject(
                HANDLE(raw as *mut c_void),
                HANDLE(child.as_raw_handle() as *mut c_void),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_aac_encode_gets_the_safe_coder() {
        let a = safe_aac(&["-i", "x", "-c:a", "aac", "-b:a", "192k", "-c:a:1", "aac", "-c", "copy", "out.mkv"]);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(
            a,
            ["-i", "x", "-c:a", "aac", "-aac_coder", "fast", "-b:a", "192k", "-c:a:1", "aac", "-aac_coder", "fast", "-c", "copy", "out.mkv"]
        );
        // Not touched: other codecs, or "aac" as a file name.
        let b = safe_aac(&["-i", "aac", "-c:a", "flac", "o.mkv"]);
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn a_stuck_ffmpeg_is_stopped() {
        init(None);
        let t = Instant::now();
        // -re reads in real time: this would take 60 s.
        let r = run_limited("ffmpeg", &["-hide_banner", "-nostdin", "-re", "-f", "lavfi", "-i", "anullsrc=d=60", "-f", "null", "-"], Some(Duration::from_secs(2)));
        assert!(r.unwrap_err().contains("stuck"));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// Replays the save that hung on 2026-10-31 (44.1 kHz mic → AAC 48 kHz)
    /// through `run`. Point REWIND_HUNG_SAVE at its .rewind-save-* folder.
    /// cargo test replays_the_hung_save -- --ignored --nocapture
    #[test]
    #[ignore]
    fn replays_the_hung_save() {
        init(None);
        let Ok(d) = std::env::var("REWIND_HUNG_SAVE") else { return };
        let d = std::path::Path::new(&d);
        let out = std::env::temp_dir().join("rewind-hung-save-replay.mkv");
        let mut a = args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "f32le", "-ar", "44100", "-ac", "1", "-i"]);
        a.push(d.join("a1.raw").into());
        a.extend(args(&["-c:a", "aac", "-b:a", "128k", "-ar", "48000"]));
        a.push(out.clone().into());
        let t = Instant::now();
        run_limited("ffmpeg", &a, Some(Duration::from_secs(60))).expect("mic track encodes");
        println!("mic track encoded in {:?}", t.elapsed());
        let _ = std::fs::remove_file(out);
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn edits_report_how_far_ffmpeg_has_got() {
        init(None);
        let out = std::env::temp_dir().join(format!("rewind-progress-{}.mkv", std::process::id()));
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<f64>::new()));
        let s = seen.clone();
        let mut a = args(&["-hide_banner", "-loglevel", "error", "-y", "-re", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30", "-t", "1.5", "-c:v", "mpeg4"]);
        a.push(out.clone().into());
        with_progress(move |secs| s.borrow_mut().push(secs), || run("ffmpeg", &a)).unwrap();
        let seen = seen.borrow();
        assert!(!seen.is_empty(), "progress was reported");
        assert!(seen.iter().all(|s| (0.0..=2.0).contains(s)), "{seen:?}");
        // Outside with_progress nothing is tracked (no -progress file).
        let mut b = args(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30", "-t", "0.2", "-c:v", "mpeg4"]);
        b.push(out.clone().into());
        run("ffmpeg", &b).unwrap();
        let _ = std::fs::remove_file(&out);
    }
}
