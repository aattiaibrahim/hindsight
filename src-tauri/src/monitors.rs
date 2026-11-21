//! Monitor list, in the same order ffmpeg's ddagrab uses.

use serde::Serialize;

#[cfg(windows)]
use windows::Win32::Foundation::POINT;
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    /// DXGI output index on the first GPU (what ddagrab's output_idx expects)
    pub index: u32,
    pub name: String,
    /// The screen's own hardware path (maker, model and the port it's plugged
    /// into). It stays the same when Windows renumbers DISPLAY1, DISPLAY2…
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

impl Monitor {
    pub fn label(&self) -> String {
        let short = self.name.trim_start_matches(r"\\.\").replace("DISPLAY", "Display ");
        format!("{short} · {}×{}", self.width, self.height)
    }
}

/// A small PNG of what's on a monitor right now (for the recording-area preview
/// in Settings): scaled to `max_w` wide on the GPU-free GDI path, cheap enough
/// for a few frames a second while that page is open.
#[cfg(windows)]
pub fn preview_png(m: &Monitor, max_w: i32) -> Result<Vec<u8>, String> {
    use windows::Win32::Graphics::Gdi::*;
    if m.width <= 0 || m.height <= 0 {
        return Err("No screen".into());
    }
    let w = max_w.min(m.width).max(16);
    let h = ((m.height as f64 * w as f64 / m.width as f64).round() as i32).max(9);
    let mut px = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        SetStretchBltMode(mem, HALFTONE);
        let ok = StretchBlt(mem, 0, 0, w, h, Some(screen), m.x, m.y, m.width, m.height, SRCCOPY);
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        SelectObject(mem, old);
        let rows = GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok.as_bool() || rows == 0 {
            return Err("Couldn't read the screen".into());
        }
    }
    // BGRA → RGB
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in px.chunks(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
#[cfg(not(windows))]
/// Linux / macOS: no preview yet (phase 3 captures the screen there).
pub fn preview_png(_m: &Monitor, _max_w: i32) -> Result<Vec<u8>, String> {
    Err("No screen preview on this system yet".into())
}

#[cfg(windows)]
pub fn list() -> Vec<Monitor> {
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        let Ok(adapter) = factory.EnumAdapters1(0) else {
            return out;
        };
        let mut i = 0;
        while let Ok(output) = adapter.EnumOutputs(i) {
            if let Ok(d) = output.GetDesc() {
                let r = d.DesktopCoordinates;
                let name = String::from_utf16_lossy(&d.DeviceName)
                    .trim_end_matches('\0')
                    .to_string();
                let id = hardware_id(&name);
                out.push(Monitor {
                    index: i,
                    name,
                    id,
                    x: r.left,
                    y: r.top,
                    width: r.right - r.left,
                    height: r.bottom - r.top,
                    primary: r.left == 0 && r.top == 0,
                });
            }
            i += 1;
        }
    }
    out
}
#[cfg(not(windows))]
/// Linux / macOS: monitors come with screen capture (phase 3).
pub fn list() -> Vec<Monitor> {
    Vec::new()
}

/// The monitor's device path for a display like \\.\DISPLAY1, e.g.
/// `\\?\DISPLAY#DELA0C0#5&2f3c&0&UID4353#{…}`; "" if Windows doesn't say.
#[cfg(windows)]
fn hardware_id(display: &str) -> String {
    use windows::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};
    use windows::core::HSTRING;
    const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
    let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    let ok = unsafe { EnumDisplayDevicesW(&HSTRING::from(display), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) };
    if !ok.as_bool() {
        return String::new();
    }
    String::from_utf16_lossy(&dd.DeviceID).trim_end_matches('\0').to_string()
}

#[cfg(windows)]
fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}
#[cfg(not(windows))]
fn cursor() -> Option<(i32, i32)> {
    None
}

/// The chosen screen: by its hardware id first (survives Windows renumbering
/// the displays), then by its DISPLAY number.
pub fn find_fixed<'a>(fixed: &str, fixed_id: &str, mons: &'a [Monitor]) -> Option<&'a Monitor> {
    (!fixed_id.is_empty())
        .then(|| mons.iter().find(|m| m.id.eq_ignore_ascii_case(fixed_id)))
        .flatten()
        .or_else(|| (fixed_id.is_empty() || mons.iter().all(|m| m.id.is_empty())).then(|| mons.iter().find(|m| m.name == fixed)).flatten())
}

static MISSING: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Which monitor to capture right now. Falls back to the primary, then to any.
pub fn pick(mode: &str, fixed: &str, fixed_id: &str, mons: &[Monitor]) -> Option<Monitor> {
    let primary = || mons.iter().find(|m| m.primary).or(mons.first()).cloned();
    match mode {
        "cursor" => cursor()
            .and_then(|(x, y)| {
                mons.iter()
                    .find(|m| x >= m.x && x < m.x + m.width && y >= m.y && y < m.y + m.height)
                    .cloned()
            })
            .or_else(primary),
        "fixed" => {
            let found = find_fixed(fixed, fixed_id, mons).cloned();
            // Say so once (not every tick) when the chosen screen isn't there.
            let note = if found.is_none() && !mons.is_empty() { format!("{fixed} {fixed_id}") } else { String::new() };
            let mut last = MISSING.lock().unwrap_or_else(|e| e.into_inner());
            if *last != note {
                if !note.is_empty() {
                    crate::log!("monitor: the chosen screen ({fixed}) isn't connected; recording the main display until it's back");
                }
                *last = note;
            }
            found.or_else(primary)
        }
        _ => primary(),
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;
    fn m(i: u32, name: &str, id: &str, primary: bool) -> Monitor {
        Monitor { index: i, name: name.into(), id: id.into(), x: i as i32 * 1920, y: 0, width: 1920, height: 1080, primary }
    }

    /// This PC's screens and their ids: cargo test --lib live_monitor_ids -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_monitor_ids() {
        let mons = list();
        for m in &mons {
            println!("{} {}x{} primary={} id={}", m.name, m.width, m.height, m.primary, m.id);
        }
        assert!(mons.iter().all(|m| !m.id.is_empty()));
    }

    #[test]
    fn fixed_screen_survives_renumbering() {
        // Chosen as DISPLAY2 (the Dell); Windows now calls the Dell DISPLAY3.
        let mons = [m(0, r"\\.\DISPLAY1", "lg", true), m(1, r"\\.\DISPLAY3", "dell", false)];
        assert_eq!(pick("fixed", r"\\.\DISPLAY2", "dell", &mons).unwrap().id, "dell");
        // The renumbered name now belongs to another screen: the id still wins.
        let mons = [m(0, r"\\.\DISPLAY2", "lg", true), m(1, r"\\.\DISPLAY1", "dell", false)];
        assert_eq!(pick("fixed", r"\\.\DISPLAY2", "dell", &mons).unwrap().id, "dell");
        // Saved before ids existed: the name.
        assert_eq!(pick("fixed", r"\\.\DISPLAY1", "", &mons).unwrap().id, "dell");
        // Unplugged: the main display.
        assert_eq!(pick("fixed", r"\\.\DISPLAY1", "benq", &mons).unwrap().id, "lg");
    }
}

#[cfg(test)]
mod preview_tests {
    /// Needs a real screen: cargo test --lib preview_is -- --ignored
    #[test]
    #[ignore]
    fn preview_is_a_small_png_and_quick() {
        let m = super::list().into_iter().find(|m| m.primary).expect("a screen");
        let t = std::time::Instant::now();
        let png = super::preview_png(&m, 960).unwrap();
        let took = t.elapsed();
        println!("preview {} KB in {took:?} for {}x{}", png.len() / 1024, m.width, m.height);
        assert_eq!(&png[1..4], b"PNG");
        assert!(took.as_millis() < 400, "{took:?}");
    }
}
