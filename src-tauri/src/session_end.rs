//! Quitting when Windows asks.
//!
//! Closing Hindsight's window keeps it running in the tray, so the replay buffer
//! keeps going. But when Windows shuts down, signs out, or an installer (an update,
//! or the installer run by hand) needs Hindsight closed, Windows asks every program's
//! windows to quit (WM_QUERYENDSESSION, then WM_ENDSESSION) through Restart Manager.
//! Hindsight never answered, so the installer said "Failed to shut down Hindsight"
//! and Event Viewer logged "Application or service 'Hindsight' could not be shut down".
//!
//! A hidden window listens for that request: it says yes, finishes a recording or a
//! clip being saved cleanly (the engine's own shutdown), and exits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_CLOSE,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::w;

static APP: OnceLock<AppHandle> = OnceLock::new();
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Finish cleanly and exit (once, whichever message comes first).
fn quit() {
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::log!("Windows asked Hindsight to close (shutting down, signing out or installing): closing");
    if let Some(app) = APP.get() {
        app.state::<crate::AppState>().engine.shutdown();
        app.exit(0);
    }
    // The app's own exit can wait for its window loop: make sure it really ends.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        std::process::exit(0);
    });
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // "May I end the session / close you?" Yes.
        WM_QUERYENDSESSION => LRESULT(1),
        // It's happening (wparam = 1): finish up now; Windows waits for this to return.
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                quit();
            }
            LRESULT(0)
        }
        // Restart Manager can also just ask the window to close.
        WM_CLOSE => {
            quit();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Start listening (a hidden top-level window on its own thread).
pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    std::thread::Builder::new()
        .name("hindsight-session-end".into())
        .spawn(|| unsafe {
            let Ok(inst) = GetModuleHandleW(None) else { return };
            let class = w!("HindsightSessionEnd");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            // Top-level (not message-only): only top-level windows are asked at shutdown.
            // Never shown.
            let Ok(_hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Hindsight"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, Some(inst.into()), None)
            else {
                return;
            };
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}
