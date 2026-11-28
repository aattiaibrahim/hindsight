//! A camera drawn into clips and recordings, like a facecam in OBS.
//!
//! One thread owns the camera (Windows Media Foundation) while something needs it:
//! the recorder, or the preview in Settings. Each new camera frame is turned here, on
//! the CPU, into the finished box: cropped (Crop to fill), mirrored, scaled to the box's
//! size on the clip, its background blurred or removed (segment.rs finds you in it),
//! with soft rounded corners and an optional thin border, as BGRA with transparency.
//! The recorder only lays that over each screen frame: on the GPU in the Native engine
//! (native/pipeline.rs), with FFmpeg's overlay in Compatibility mode (engine.rs).
//!
//! The camera is only open while it's used: turned off in Settings, or nothing
//! asking for frames for a few seconds, and it's closed (its light goes off).

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the box looks and where it goes, from Settings (changed live, no restart).
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub enabled: bool,
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub mirror: bool,
    pub border: bool,
    pub fill: bool,
    /// "off" | "blur" | "remove"
    pub background: String,
}

impl Layout {
    pub fn of(s: &crate::settings::Settings) -> Layout {
        Layout {
            enabled: s.camera_enabled,
            id: s.camera_id.clone(),
            name: s.camera_name.clone(),
            x: s.camera_x,
            y: s.camera_y,
            size: s.camera_size,
            mirror: s.camera_mirror,
            border: s.camera_border,
            fill: s.camera_fill,
            background: s.camera_background.clone(),
        }
    }

    fn effect(&self) -> bool {
        self.background == "blur" || self.background == "remove"
    }
}

/// The finished box: BGRA with straight (not premultiplied) transparency.
pub struct Boxed {
    /// Changes whenever the picture does.
    pub seq: u64,
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

#[derive(Serialize, Clone)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
}

struct Raw {
    w: u32,
    h: u32,
    bgra: Vec<u8>,
}

struct Shared {
    layout: Option<Layout>,
    /// Last time something asked for frames.
    wanted: Option<Instant>,
    /// The box size the recorder wants, in screen pixels (0 = whatever the preview wants).
    size: (u32, u32),
    raw: Option<Arc<Raw>>,
    /// Where you are in `raw` (segment::W × H), when a background effect is on.
    mask: Option<Arc<Vec<f32>>>,
    boxed: Option<Arc<Boxed>>,
    /// Why the camera couldn't be used, if it couldn't.
    error: Option<String>,
    /// Something to say that isn't a failure (the background effect getting ready).
    note: Option<String>,
    running: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    layout: None,
    wanted: None,
    size: (0, 0),
    raw: None,
    mask: None,
    boxed: None,
    error: None,
    note: None,
    running: false,
});
static SEQ: AtomicU64 = AtomicU64::new(1);
/// How long the camera stays open after the last request.
const LINGER: Duration = Duration::from_secs(4);

/// Settings changed: the box moves or changes at once; a new camera (or off) is picked up by the thread.
pub fn apply(s: &crate::settings::Settings) {
    let l = Layout::of(s);
    let mut g = SHARED.lock().unwrap();
    // Moving or resizing the box keeps the picture; a new look is made from the next frame.
    let look = |x: &Layout| (x.enabled, x.id.clone(), x.mirror, x.border, x.fill, x.background.clone());
    if g.layout.as_ref().is_none_or(|old| look(old) != look(&l)) {
        g.boxed = None;
    }
    if !l.effect() {
        g.mask = None;
        g.note = None;
    }
    g.layout = Some(l);
}

pub fn layout() -> Option<Layout> {
    SHARED.lock().unwrap().layout.clone().filter(|l| l.enabled)
}

/// The camera's picture shape (width / height) once it's open, as the box will be.
pub fn aspect() -> Option<f64> {
    let g = SHARED.lock().unwrap();
    let l = g.layout.as_ref()?;
    let raw = g.raw.as_ref()?;
    Some(if l.fill { 4.0 / 3.0 } else { raw.w as f64 / raw.h.max(1) as f64 })
}

/// The recorder asks for the box at this size (screen pixels), every frame.
/// Returns the newest finished one at that size, if there is one yet.
pub fn want(w: u32, h: u32) -> Option<Arc<Boxed>> {
    let mut g = SHARED.lock().unwrap();
    g.wanted = Some(Instant::now());
    g.size = (w, h);
    ensure_thread(&mut g);
    g.boxed.clone().filter(|b| b.w == w && b.h == h)
}

/// For Settings: the background effect's progress or problem, if there's one to show.
pub fn note() -> Option<String> {
    SHARED.lock().unwrap().note.clone()
}

/// The Settings preview: the box as it looks (at a small size), as a PNG.
pub fn preview_png() -> Result<Vec<u8>, String> {
    let (raw, layout, mask) = {
        let mut g = SHARED.lock().unwrap();
        g.wanted = Some(Instant::now());
        ensure_thread(&mut g);
        if let Some(e) = &g.error {
            return Err(e.clone());
        }
        (g.raw.clone(), g.layout.clone(), g.mask.clone())
    };
    let (Some(raw), Some(l)) = (raw, layout) else { return Err("starting".into()) };
    let aspect = if l.fill { 4.0 / 3.0 } else { raw.w as f64 / raw.h.max(1) as f64 };
    let w = 320u32;
    let h = ((w as f64 / aspect).round() as u32).max(2);
    let mut rgba = make_box(&raw, &l, w, h, mask.as_deref().map(|m| m.as_slice())).bgra;
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn ensure_thread(g: &mut Shared) {
    if g.running || !g.layout.as_ref().is_some_and(|l| l.enabled) {
        return;
    }
    g.running = true;
    g.error = None;
    let _ = std::thread::Builder::new().name("hindsight-camera".into()).spawn(run);
}

/// Wanted and turned on: keep the camera open, else close it and end.
fn still_wanted() -> Option<Layout> {
    let g = SHARED.lock().unwrap();
    let on = g.layout.clone().filter(|l| l.enabled)?;
    g.wanted.filter(|t| t.elapsed() < LINGER)?;
    Some(on)
}

/// The background model, loaded (and if needed downloaded) on another thread the
/// first time an effect is on, so the camera keeps showing meanwhile.
enum Seg {
    None,
    Loading(std::sync::mpsc::Receiver<Result<crate::segment::Segmenter, String>>),
    Ready(crate::segment::Segmenter),
    Failed(Instant),
}

fn run() {
    let mut fails = 0;
    let mut seg = Seg::None;
    loop {
        let Some(l) = still_wanted() else { break };
        let opened_as = l.id.clone();
        let r = device::read(&l.id, &l.name, &mut |w, h, bgra| {
            let Some(now) = still_wanted().filter(|n| n.id == opened_as) else {
                return false; // turned off, unused, or another camera picked
            };
            let raw = Arc::new(Raw { w, h, bgra });
            let mask = if now.effect() { segment(&mut seg, &raw) } else { None };
            let (layout, size) = {
                let mut g = SHARED.lock().unwrap();
                g.raw = Some(raw.clone());
                g.mask = mask.clone();
                g.error = None;
                (g.layout.clone(), g.size)
            };
            // The recorder's size; the preview makes its own small one.
            if let Some(l) = layout
                && size.0 >= 8
                && size.1 >= 8
            {
                let b = Arc::new(make_box(&raw, &l, size.0, size.1, mask.as_deref().map(|m| m.as_slice())));
                SHARED.lock().unwrap().boxed = Some(b);
            }
            fails = 0;
            true
        });
        if let Err(e) = r {
            fails += 1;
            crate::log!("Camera: {e}");
            SHARED.lock().unwrap().error = Some(e);
            // Busy in another app, unplugged…: try again now and then while it's wanted.
            std::thread::sleep(Duration::from_secs(if fails < 3 { 2 } else { 10 }));
        }
    }
    let mut g = SHARED.lock().unwrap();
    g.running = false;
    g.raw = None;
    g.mask = None;
    g.boxed = None;
}

/// Where you are in this frame, once the model is ready (None until then: no effect yet).
fn segment(seg: &mut Seg, raw: &Raw) -> Option<Arc<Vec<f32>>> {
    let note = |n: Option<&str>| SHARED.lock().unwrap().note = n.map(str::to_string);
    match seg {
        Seg::None => {
            note(Some("Getting the background effect ready (a one-time download of about 0.5 MB)…"));
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().name("hindsight-segment".into()).spawn(move || {
                let _ = tx.send(crate::segment::Segmenter::new());
            });
            *seg = Seg::Loading(rx);
            None
        }
        Seg::Loading(rx) => match rx.try_recv() {
            Ok(Ok(s)) => {
                note(None);
                *seg = Seg::Ready(s);
                segment(seg, raw)
            }
            Ok(Err(e)) => {
                crate::log!("Camera background: {e}");
                note(Some(&format!("The background effect isn't available: {e}")));
                *seg = Seg::Failed(Instant::now());
                None
            }
            Err(_) => None,
        },
        Seg::Ready(s) => match s.mask(&raw.bgra, raw.w, raw.h) {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                note(Some(&format!("The background effect stopped: {e}")));
                *seg = Seg::Failed(Instant::now());
                None
            }
        },
        // Try again a minute later (no internet at first, say).
        Seg::Failed(at) => {
            if at.elapsed() > Duration::from_secs(60) {
                *seg = Seg::None;
            }
            None
        }
    }
}

pub fn list() -> Vec<CameraInfo> {
    device::list()
}

/// Tests: a made-up camera picture instead of a real camera (no thread, no device).
#[cfg(test)]
pub fn test_feed(l: Layout, w: u32, h: u32, bgra: Vec<u8>) {
    let mut g = SHARED.lock().unwrap();
    g.layout = Some(l);
    g.running = true;
    g.raw = Some(Arc::new(Raw { w, h, bgra }));
    g.boxed = None;
}

/// Tests: make the box at the size last asked for, as the camera thread would.
#[cfg(test)]
pub fn test_make() {
    let mut g = SHARED.lock().unwrap();
    let (raw, l, (w, h)) = (g.raw.clone().unwrap(), g.layout.clone().unwrap(), g.size);
    g.boxed = Some(Arc::new(make_box(&raw, &l, w, h, None)));
}

/// A soft step from 0 to 1 between `a` and `b`: firm edges around you, no hard stairs.
fn smooth(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A blur of `src` (`w` × `h` BGRA, transparency left alone): three box blurs
/// across and down, which together look like a Gaussian blur.
fn blur(src: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut a = src.to_vec();
    let mut b = src.to_vec();
    for _ in 0..3 {
        pass(&a, &mut b, w, h, r, true);
        pass(&b, &mut a, w, h, r, false);
    }
    a
}

/// One box blur of radius `r`, across (`rows`) or down, as a running sum.
fn pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, rows: bool) {
    let (len, lines) = if rows { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if rows { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let n = (2 * r + 1) as u32;
    for line in 0..lines {
        for c in 0..3 {
            let at = |i: isize| src[idx(line, i.clamp(0, len as isize - 1) as usize) + c] as u32;
            let mut sum: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for i in 0..len {
                dst[idx(line, i) + c] = (sum / n) as u8;
                sum = sum + at(i as isize + r as isize + 1) - at(i as isize - r as isize);
            }
        }
    }
}

/// The finished box at `w` × `h`: the camera picture cropped to the box's shape
/// (centred), mirrored if asked, scaled by averaging (sharp when shrinking); with
/// `mask` (where you are), the background blurred or made see-through; then soft
/// rounded corners and a thin border along them if asked (not with Remove: no box then).
fn make_box(raw: &Raw, l: &Layout, w: u32, h: u32, mask: Option<&[f32]>) -> Boxed {
    // The part of the camera picture with the box's shape.
    let want = w as f64 / h as f64;
    let have = raw.w as f64 / raw.h.max(1) as f64;
    let (cw, ch) = if have > want { (raw.h as f64 * want, raw.h as f64) } else { (raw.w as f64, raw.w as f64 / want) };
    let (cx, cy) = ((raw.w as f64 - cw) / 2.0, (raw.h as f64 - ch) / 2.0);
    let (sx, sy) = (cw / w as f64, ch / h as f64);
    let mut out = vec![0u8; (w * h * 4) as usize];
    let stride = (raw.w * 4) as usize;
    for y in 0..h {
        let y0 = (cy + y as f64 * sy) as u32;
        let y1 = ((cy + (y + 1) as f64 * sy) as u32).max(y0 + 1).min(raw.h);
        for x in 0..w {
            let xo = if l.mirror { w - 1 - x } else { x };
            let x0 = (cx + xo as f64 * sx) as u32;
            let x1 = ((cx + (xo + 1) as f64 * sx) as u32).max(x0 + 1).min(raw.w);
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            // At most 4×4 samples per pixel: plenty for a facecam, and cheap.
            let (stepx, stepy) = (((x1 - x0) / 4).max(1), ((y1 - y0) / 4).max(1));
            let mut yy = y0;
            while yy < y1 {
                let row = yy as usize * stride;
                let mut xx = x0;
                while xx < x1 {
                    let i = row + xx as usize * 4;
                    b += raw.bgra[i] as u32;
                    g += raw.bgra[i + 1] as u32;
                    r += raw.bgra[i + 2] as u32;
                    n += 1;
                    xx += stepx;
                }
                yy += stepy;
            }
            let o = ((y * w + x) * 4) as usize;
            let n = n.max(1);
            out[o] = (b / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (r / n) as u8;
            out[o + 3] = 255;
        }
    }
    let remove = mask.is_some() && l.background == "remove";
    if let Some(m) = mask {
        // How much of each pixel is you, looked up where it came from in the camera picture.
        let you = |x: u32, y: u32| {
            let xo = if l.mirror { w - 1 - x } else { x };
            let fx = (cx + (xo as f64 + 0.5) * sx) / raw.w as f64;
            let fy = (cy + (y as f64 + 0.5) * sy) / raw.h as f64;
            smooth(0.35, 0.65, crate::segment::at(m, fx, fy))
        };
        if remove {
            for y in 0..h {
                for x in 0..w {
                    out[((y * w + x) * 4 + 3) as usize] = (you(x, y) * 255.0).round() as u8;
                }
            }
        } else {
            let soft = blur(&out, w as usize, h as usize, ((h as f64 / 28.0).round() as usize).max(3));
            for y in 0..h {
                for x in 0..w {
                    let t = you(x, y);
                    let o = ((y * w + x) * 4) as usize;
                    for c in 0..3 {
                        out[o + c] = (out[o + c] as f32 * t + soft[o + c] as f32 * (1.0 - t)).round() as u8;
                    }
                }
            }
        }
    }
    if !remove {
        let radius = ((w.min(h) as f64) * 0.12).round() as u32;
        let t = ((h as f64 / 110.0).round() as u32).max(2) as f64;
        for y in 0..h {
            for x in 0..w {
                let d = edge_distance(x, y, w, h, radius);
                let o = ((y * w + x) * 4) as usize;
                if l.border && d < t {
                    for c in 0..3 {
                        out[o + c] = ((out[o + c] as u32 + 3 * 240) / 4) as u8;
                    }
                }
                // Soft corners: a pixel half outside the curve is half see-through.
                out[o + 3] = ((d + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    Boxed { seq: SEQ.fetch_add(1, Ordering::Relaxed), w, h, bgra: out }
}

/// How far a pixel is inside the rounded box's edge.
fn edge_distance(x: u32, y: u32, w: u32, h: u32, r: u32) -> f64 {
    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
    let (w, h, r) = (w as f64, h as f64, r as f64);
    let straight = px.min(w - px).min(py).min(h - py);
    // In a corner square: distance to the arc instead.
    let cx = if px < r { Some(r) } else if px > w - r { Some(w - r) } else { None };
    let cy = if py < r { Some(r) } else if py > h - r { Some(h - r) } else { None };
    match (cx, cy) {
        (Some(cx), Some(cy)) => r - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt(),
        _ => straight,
    }
}

#[cfg(windows)]
mod device {
    //! Media Foundation: list cameras, read frames as BGRA.
    use super::CameraInfo;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
    use windows::core::{GUID, Interface, PWSTR};

    fn start() -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string())
        }
    }

    fn string(a: &IMFActivate, key: &GUID) -> String {
        unsafe {
            let mut p = PWSTR::null();
            let mut n = 0u32;
            if a.GetAllocatedString(key, &mut p, &mut n).is_err() || p.is_null() {
                return String::new();
            }
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    fn devices() -> Result<Vec<IMFActivate>, String> {
        start()?;
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("no attributes")?;
            attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).map_err(|e| e.to_string())?;
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFEnumDeviceSources(&attrs, &mut list, &mut count).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for i in 0..count as usize {
                if let Some(a) = (*list.add(i)).take() {
                    out.push(a);
                }
            }
            if !list.is_null() {
                CoTaskMemFree(Some(list as *const _));
            }
            Ok(out)
        }
    }

    pub fn list() -> Vec<CameraInfo> {
        devices()
            .unwrap_or_default()
            .iter()
            .map(|a| CameraInfo {
                id: string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK),
                name: string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME),
            })
            .collect()
    }

    const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    /// Open the camera (by its link, else its name, else the first one) and hand each
    /// frame to `frame` as BGRA rows, top first, until it returns false.
    pub fn read(id: &str, name: &str, frame: &mut dyn FnMut(u32, u32, Vec<u8>) -> bool) -> Result<(), String> {
        let all = devices()?;
        let pick = all
            .iter()
            .find(|a| !id.is_empty() && string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK) == id)
            .or_else(|| all.iter().find(|a| !name.is_empty() && string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME) == name))
            .or_else(|| if id.is_empty() && name.is_empty() { all.first() } else { None })
            .ok_or("The camera isn't connected.")?;
        unsafe {
            let source: IMFMediaSource = pick.ActivateObject().map_err(|e| format!("Couldn't open the camera (is another app using it?): {e}"))?;
            let result = read_from(&source, frame);
            let _ = source.Shutdown();
            result
        }
    }

    unsafe fn read_from(source: &IMFMediaSource, frame: &mut dyn FnMut(u32, u32, Vec<u8>) -> bool) -> Result<(), String> {
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("no attributes")?;
            // Decodes MJPG and converts to RGB for us.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
            let reader = MFCreateSourceReaderFromMediaSource(source, &attrs).map_err(|e| e.to_string())?;
            // The camera's own format: up to 1280×720 is plenty for a box, at the best frame rate.
            let mut best: Option<(IMFMediaType, u32, u32, f64)> = None;
            let mut i = 0;
            while let Ok(t) = reader.GetNativeMediaType(STREAM, i) {
                i += 1;
                let (w, h) = unpack(t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0));
                let (n, d) = unpack(t.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0));
                let fps = if d > 0 { n as f64 / d as f64 } else { 0.0 };
                if w == 0 || h == 0 || w > 1920 {
                    continue;
                }
                let score = |w: u32, h: u32, fps: f64| (fps.min(30.0) * 10.0).round() as i64 * 10_000_000 - ((w * h) as i64 - 1280 * 720).abs();
                if best.as_ref().is_none_or(|b| score(w, h, fps) > score(b.1, b.2, b.3)) {
                    best = Some((t, w, h, fps));
                }
            }
            let (native, w, h, _) = best.ok_or("The camera offered no picture format Hindsight can use.")?;
            reader.SetCurrentMediaType(STREAM, None, &native).map_err(|e| e.to_string())?;
            let out = MFCreateMediaType().map_err(|e| e.to_string())?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| e.to_string())?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64).map_err(|e| e.to_string())?;
            reader.SetCurrentMediaType(STREAM, None, &out).map_err(|e| format!("The camera's picture couldn't be converted: {e}"))?;
            let cur = reader.GetCurrentMediaType(STREAM).map_err(|e| e.to_string())?;
            let (w, h) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(((w as u64) << 32) | h as u64));
            let default_stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((w * 4) as i32);
            let mut quiet = 0;
            loop {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(STREAM, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .map_err(|e| format!("The camera stopped: {e}"))?;
                if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 | MF_SOURCE_READERF_ERROR.0 as u32) != 0 {
                    return Err("The camera stopped (unplugged?).".into());
                }
                let Some(sample) = sample else {
                    quiet += 1;
                    if quiet > 200 {
                        return Err("The camera sends no picture.".into());
                    }
                    continue;
                };
                quiet = 0;
                let buf = sample.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
                let mut bgra = vec![0u8; (w * h * 4) as usize];
                let row = (w * 4) as usize;
                if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
                    let mut p: *mut u8 = std::ptr::null_mut();
                    let mut pitch = 0i32;
                    b2.Lock2D(&mut p, &mut pitch).map_err(|e| e.to_string())?;
                    for y in 0..h as isize {
                        let src = p.offset(y * pitch as isize);
                        std::ptr::copy_nonoverlapping(src, bgra.as_mut_ptr().add(y as usize * row), row);
                    }
                    let _ = b2.Unlock2D();
                } else {
                    let mut p: *mut u8 = std::ptr::null_mut();
                    let mut len = 0u32;
                    buf.Lock(&mut p, None, Some(&mut len)).map_err(|e| e.to_string())?;
                    if (len as usize) >= row * h as usize {
                        for y in 0..h as usize {
                            // A negative stride: the rows come bottom first.
                            let from = if default_stride < 0 { (h as usize - 1 - y) * row } else { y * row };
                            std::ptr::copy_nonoverlapping(p.add(from), bgra.as_mut_ptr().add(y * row), row);
                        }
                    }
                    let _ = buf.Unlock();
                }
                if !frame(w, h, bgra) {
                    return Ok(());
                }
            }
        }
    }

    fn unpack(v: u64) -> (u32, u32) {
        ((v >> 32) as u32, v as u32)
    }
}

#[cfg(not(windows))]
mod device {
    use super::CameraInfo;
    pub fn list() -> Vec<CameraInfo> {
        Vec::new()
    }
    pub fn read(_id: &str, _name: &str, _frame: &mut dyn FnMut(u32, u32, Vec<u8>) -> bool) -> Result<(), String> {
        Err("Cameras aren't supported on this system yet.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout { enabled: true, id: String::new(), name: String::new(), x: 0.7, y: 0.7, size: 0.2, mirror: false, border: false, fill: false, background: "off".into() }
    }

    /// Left half red, right half blue.
    fn raw(w: u32, h: u32) -> Raw {
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if x < w / 2 { bgra[i + 2] = 255 } else { bgra[i] = 255 }
                bgra[i + 3] = 255;
            }
        }
        Raw { w, h, bgra }
    }

    fn px(b: &Boxed, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * b.w + x) * 4) as usize;
        [b.bgra[i], b.bgra[i + 1], b.bgra[i + 2]]
    }

    #[test]
    fn scales_and_mirrors() {
        let r = raw(1280, 720);
        let b = make_box(&r, &layout(), 320, 180, None);
        assert_eq!((b.w, b.h), (320, 180));
        assert_eq!(px(&b, 10, 90), [0, 0, 255], "left stays red");
        let m = make_box(&r, &Layout { mirror: true, ..layout() }, 320, 180, None);
        assert_eq!(px(&m, 10, 90), [255, 0, 0], "mirrored: left is blue");
    }

    #[test]
    fn crop_to_fill_keeps_the_middle() {
        // A 4:3 box from a 16:9 picture cuts the sides, keeping both colours.
        let b = make_box(&raw(1600, 900), &Layout { fill: true, ..layout() }, 400, 300, None);
        assert_eq!(px(&b, 5, 150), [0, 0, 255]);
        assert_eq!(px(&b, 394, 150), [255, 0, 0]);
    }

    #[test]
    fn border_lightens_the_edge_only() {
        let b = make_box(&raw(1280, 720), &Layout { border: true, ..layout() }, 320, 180, None);
        assert!(px(&b, 160, 1)[1] > 150, "edge is light");
        assert_eq!(px(&b, 100, 90), [0, 0, 255], "middle untouched");
    }

    fn alpha(b: &Boxed, x: u32, y: u32) -> u8 {
        b.bgra[((y * b.w + x) * 4 + 3) as usize]
    }

    #[test]
    fn soft_rounded_corners() {
        let b = make_box(&raw(1280, 720), &layout(), 320, 180, None);
        assert_eq!(alpha(&b, 0, 0), 0, "corner see-through");
        assert_eq!(alpha(&b, 319, 179), 0);
        assert_eq!(alpha(&b, 0, 90), 255, "straight edge solid");
        assert_eq!(alpha(&b, 160, 90), 255);
        let edge: Vec<u8> = (0..30).map(|i| alpha(&b, i, i)).collect();
        assert!(edge.iter().any(|a| *a > 0 && *a < 255), "the curve is soft: {edge:?}");
    }

    /// You on the right half of the picture.
    fn right_half() -> Vec<f32> {
        use crate::segment::{H, W};
        (0..W * H).map(|i| if i % W >= W / 2 { 1.0 } else { 0.0 }).collect()
    }

    #[test]
    fn remove_makes_the_background_see_through() {
        let b = make_box(&raw(1280, 720), &Layout { background: "remove".into(), border: true, ..layout() }, 320, 180, Some(&right_half()));
        assert_eq!(alpha(&b, 20, 90), 0, "background gone");
        assert_eq!(alpha(&b, 300, 90), 255, "you stay");
        assert_eq!(px(&b, 300, 90), [255, 0, 0], "no border without a box");
        // Mirrored, you move to the left with the picture.
        let m = make_box(&raw(1280, 720), &Layout { background: "remove".into(), mirror: true, ..layout() }, 320, 180, Some(&right_half()));
        assert_eq!(alpha(&m, 20, 90), 255);
        assert_eq!(alpha(&m, 300, 90), 0);
    }

    #[test]
    fn blur_softens_only_the_background() {
        // Fine stripes everywhere; you on the right.
        let (w, h) = (1280u32, 720u32);
        let mut bgra = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 8) % 2 == 0 { 0 } else { 255 };
                let i = ((y * w + x) * 4) as usize;
                bgra[i..i + 3].copy_from_slice(&[v, v, v]);
            }
        }
        let b = make_box(&Raw { w, h, bgra }, &Layout { background: "blur".into(), ..layout() }, 640, 360, Some(&right_half()));
        let spread = |x0: u32| {
            let vals: Vec<u8> = (x0..x0 + 40).map(|x| px(&b, x, 180)[0]).collect();
            *vals.iter().max().unwrap() as i32 - *vals.iter().min().unwrap() as i32
        };
        assert!(spread(60) < 60, "background blurred: {}", spread(60));
        assert!(spread(520) > 200, "you stay sharp: {}", spread(520));
        assert_eq!(alpha(&b, 320, 180), 255, "nothing see-through with blur");
    }

    /// What the camera costs: CPU time over 10 s with the recorder asking for a
    /// 1080p clip's box 60 times a second, with no background effect and with blur.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn real_camera_cost() {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let cpu = || unsafe {
            let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u).unwrap();
            let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
            t(k) + t(u)
        };
        crate::game_detect::init(std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.rewind.app").join("ai"));
        for bg in ["off", "blur", "remove"] {
            let s = crate::settings::Settings { camera_enabled: true, camera_background: bg.into(), camera_border: true, ..Default::default() };
            apply(&s);
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(6) {
                want(422, 236);
                std::thread::sleep(Duration::from_millis(16));
            }
            let (c0, t0) = (cpu(), Instant::now());
            let mut frames = 0u64;
            let mut last = 0;
            while t0.elapsed() < Duration::from_secs(10) {
                if let Some(b) = want(422, 236)
                    && b.seq != last
                {
                    last = b.seq;
                    frames += 1;
                }
                std::thread::sleep(Duration::from_millis(16));
            }
            let used = cpu() - c0;
            let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            eprintln!(
                "{bg}: {:.0}% of one core ({:.1}% of all {cores}), {:.1} camera frames/s, note {:?}",
                used / 10.0 * 100.0,
                used / 10.0 * 100.0 / cores as f64,
                frames as f64 / 10.0,
                note()
            );
        }
    }

    /// A real camera, if there is one: list it and read a few frames.
    #[test]
    #[ignore]
    fn real_camera() {
        let cams = list();
        eprintln!("cameras: {:?}", cams.iter().map(|c| &c.name).collect::<Vec<_>>());
        let Some(c) = cams.first() else { return };
        let mut n = 0;
        device::read(&c.id, &c.name, &mut |w, h, bgra| {
            eprintln!("frame {w}x{h} {} bytes, first px {:?}", bgra.len(), &bgra[..4]);
            n += 1;
            n < 5
        })
        .unwrap();
    }
}
