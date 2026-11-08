//! Monitor list, in the same order ffmpeg's ddagrab uses.

use serde::Serialize;

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    /// DXGI output index on the first GPU (what ddagrab's output_idx expects)
    pub index: u32,
    pub name: String,
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
                out.push(Monitor {
                    index: i,
                    name,
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

fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// Which monitor to capture right now. Falls back to the primary, then to any.
pub fn pick(mode: &str, fixed: &str, mons: &[Monitor]) -> Option<Monitor> {
    let primary = || mons.iter().find(|m| m.primary).or(mons.first()).cloned();
    match mode {
        "cursor" => cursor()
            .and_then(|(x, y)| {
                mons.iter()
                    .find(|m| x >= m.x && x < m.x + m.width && y >= m.y && y < m.y + m.height)
                    .cloned()
            })
            .or_else(primary),
        "fixed" => mons.iter().find(|m| m.name == fixed).cloned().or_else(primary),
        _ => primary(),
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
