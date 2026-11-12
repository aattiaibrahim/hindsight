//! Which programs have sound open, and on which output device, for diagnostics.
//!
//! "Desktop audio" is recorded from Windows' default output. A game set to play
//! to another device (a headset picked in the game, or in Windows' per-app
//! "Volume mixer" settings) can be missing from clips, and this is how a
//! diagnostics file shows it.

use std::collections::HashMap;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::core::Interface;

/// One output device and the programs with sound open on it.
pub struct Output {
    pub name: String,
    pub default: bool,
    /// (program, playing right now)
    pub apps: Vec<(String, bool)>,
}

fn process_names() -> HashMap<u32, String> {
    let mut out = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.insert(e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..len]));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    out
}

/// Every active output device with its programs (on its own thread, so COM is ours).
pub fn outputs() -> Vec<Output> {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let out = list().unwrap_or_default();
        CoUninitialize();
        out
    })
    .join()
    .unwrap_or_default()
}

unsafe fn id_of(d: &IMMDevice) -> Option<String> {
    unsafe {
        let p = d.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

unsafe fn list() -> windows::core::Result<Vec<Output>> {
    unsafe {
        let names = process_names();
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default_id = en.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| id_of(&d));
        let all = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..all.GetCount()? {
            let Ok(d) = all.Item(i) else { continue };
            let name = d
                .OpenPropertyStore(STGM_READ)
                .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
                .and_then(|v| PropVariantToStringAlloc(&v))
                .map(|p| {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as _));
                    s
                })
                .unwrap_or_else(|_| "(unnamed output)".into());
            let mut apps: Vec<(String, bool)> = Vec::new();
            if let Ok(mgr) = d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                if let Ok(sessions) = mgr.GetSessionEnumerator() {
                    for j in 0..sessions.GetCount().unwrap_or(0) {
                        let Ok(c) = sessions.GetSession(j) else { continue };
                        let Ok(c2) = c.cast::<IAudioSessionControl2>() else { continue };
                        let pid = c2.GetProcessId().unwrap_or(0);
                        if pid == 0 {
                            continue; // Windows' own sounds
                        }
                        let playing = c.GetState().map(|s| s == AudioSessionStateActive).unwrap_or(false);
                        let exe = names.get(&pid).cloned().unwrap_or_else(|| format!("process {pid}"));
                        match apps.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&exe)) {
                            Some(a) => a.1 |= playing,
                            None => apps.push((exe, playing)),
                        }
                    }
                }
            }
            apps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            out.push(Output { default: id_of(&d).is_some() && id_of(&d) == default_id, name, apps });
        }
        out.sort_by(|a, b| b.default.cmp(&a.default));
        Ok(out)
    }
}

/// The diagnostics lines: one per output, and a warning for programs playing elsewhere.
pub fn report_lines(outputs: &[Output]) -> Vec<String> {
    let mut lines = Vec::new();
    for o in outputs {
        let apps = if o.apps.is_empty() {
            "nothing".to_string()
        } else {
            o.apps.iter().map(|(n, p)| if *p { format!("{n} (playing)") } else { n.clone() }).collect::<Vec<_>>().join(", ")
        };
        lines.push(format!("Output{}: {} - {}", if o.default { " (default)" } else { "" }, o.name, apps));
    }
    let elsewhere: Vec<String> = outputs
        .iter()
        .filter(|o| !o.default)
        .flat_map(|o| o.apps.iter().filter(|(_, p)| *p).map(move |(n, _)| format!("{n} on {}", o.name)))
        .collect();
    if !elsewhere.is_empty() {
        lines.push(format!(
            "Warning: playing to an output that isn't the default: {}. Desktop audio is recorded from the default output, so this sound may be missing from clips.",
            elsewhere.join("; ")
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_on_another_output_is_called_out() {
        let outs = vec![
            Output { name: "Speakers".into(), default: true, apps: vec![("chrome.exe".into(), true)] },
            Output { name: "Headset".into(), default: false, apps: vec![("VALORANT-Win64-Shipping.exe".into(), true), ("Discord.exe".into(), false)] },
        ];
        let l = report_lines(&outs);
        assert_eq!(l[0], "Output (default): Speakers - chrome.exe (playing)");
        assert!(l[2].contains("VALORANT-Win64-Shipping.exe on Headset") && !l[2].contains("Discord"), "{l:?}");
    }

    /// This PC: cargo test live_audio_outputs -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_audio_outputs() {
        for l in report_lines(&outputs()) {
            println!("{l}");
        }
    }
}
