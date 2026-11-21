//! Starting with Windows, made sure.
//!
//! Windows starts Hindsight at sign-in (`--minimized`), while it's still setting up
//! the desktop: WebView2, the GPU driver and audio devices may not be ready yet, and
//! a start that fails then is gone without a trace (on PCs with Windows Error
//! Reporting turned off, nothing at all is logged). So:
//!  - every start, and anything that stops it early, goes in `logs\startup.log`
//!    (always, even with the detailed log off), and "ready" once it's up;
//!  - a start at sign-in that fails tries again 15 s later, up to three times.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

const RETRY: &str = "--startup-retry=";
const MAX_RETRIES: u32 = 3;

/// `%APPDATA%\com.rewind.app\logs\startup.log`, the app's own logs folder.
fn log_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("com.rewind.app").join("logs").join("startup.log"))
}

/// One line in the startup log (kept small: it starts over past 64 KB).
pub fn note(msg: &str) {
    let Some(f) = log_file() else { return };
    let _ = std::fs::create_dir_all(f.parent().unwrap_or(&f));
    if std::fs::metadata(&f).is_ok_and(|m| m.len() > 64 * 1024) {
        let _ = std::fs::remove_file(&f);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(file, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"));
    }
}

fn args() -> Vec<String> {
    std::env::args().skip(1).collect()
}

/// Started by Windows at sign-in (the autostart entry passes --minimized).
fn at_sign_in(args: &[String]) -> bool {
    args.iter().any(|a| a == "--minimized")
}

fn retries(args: &[String]) -> u32 {
    args.iter().find_map(|a| a.strip_prefix(RETRY)?.parse().ok()).unwrap_or(0)
}

/// First thing in `run`: note the start, wait before a retry, and catch a crash.
pub fn begin() {
    let a = args();
    note(&format!("starting Hindsight {} (pid {}, {})", env!("CARGO_PKG_VERSION"), std::process::id(), if a.is_empty() { "no arguments".into() } else { a.join(" ") }));
    if retries(&a) > 0 {
        // The copy that failed has gone, and Windows has had time to finish signing in.
        std::thread::sleep(Duration::from_secs(15));
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        note(&format!("stopped with an error: {info}"));
        try_again();
        previous(info);
    }));
}

/// Up and running.
pub fn ready() {
    note("ready");
}

/// A start at sign-in that failed: start again in 15 s (a fresh copy, which waits
/// first), at most three times.
pub fn try_again() {
    let a = args();
    if !at_sign_in(&a) {
        return;
    }
    let n = retries(&a);
    if n >= MAX_RETRIES {
        note("gave up starting after 3 tries; start Hindsight from the Start menu");
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut next: Vec<String> = a.into_iter().filter(|x| !x.starts_with(RETRY)).collect();
    next.push(format!("{RETRY}{}", n + 1));
    match std::process::Command::new(exe).args(&next).spawn() {
        Ok(_) => note(&format!("trying again in 15 s (try {} of {MAX_RETRIES})", n + 1)),
        Err(e) => note(&format!("couldn't start again: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_only_at_sign_in_and_count_up() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(at_sign_in(&s(&["--minimized"])));
        assert!(!at_sign_in(&s(&["--save-clip", "30"])));
        assert_eq!(retries(&s(&["--minimized"])), 0);
        assert_eq!(retries(&s(&["--minimized", "--startup-retry=2"])), 2);
    }
}
