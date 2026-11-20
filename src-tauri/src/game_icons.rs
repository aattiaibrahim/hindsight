//! Game logos for the library.
//!
//! First choice (when "Online game logos" is on): the game's official icon
//! from Steam. Only the folder's name is sent (to Steam's store search), and
//! only for folders that look like a game; the icon is downloaded once and
//! kept. A Steam match has to carry the same name, so "Edit" or "funny clips"
//! never turn into some game's logo.
//!
//! Otherwise, the game's own program icon, as a small PNG. Where the program
//! comes from, best first:
//! 1. The program you were playing when a clip was saved (remembered from
//!    game detection, so every game you record gets its icon).
//! 2. Epic's install records (they name the game's program).
//! 3. Steam's install records: the game's folder, searched for its program.
//! 4. Riot's install folders (VALORANT, League of Legends, …).
//! A game with neither gets a letter tile in the app instead.

use crate::consolidate::game_key;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
/// game key → program path
static EXES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Where icons are kept (app data). Set once at startup.
pub fn init(dir: PathBuf) {
    let _ = fs::create_dir_all(&dir);
    let map = fs::read_to_string(dir.join("programs.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    *EXES.lock().unwrap() = Some(map);
    *DIR.lock().unwrap() = Some(dir);
}

/// Game detection saw `game` running as `exe`: remember it for its icon.
pub fn remember(game: &str, exe: &str) {
    let key = game_key(game);
    let mut g = EXES.lock().unwrap();
    let Some(map) = g.as_mut() else { return };
    if map.get(&key).map(String::as_str) == Some(exe) {
        return;
    }
    map.insert(key, exe.to_string());
    if let (Some(dir), Ok(t)) = (DIR.lock().unwrap().clone(), serde_json::to_string(map)) {
        let _ = fs::write(dir.join("programs.json"), t);
    }
}

const NOT_THE_GAME: [&str; 14] = [
    "unins", "crash", "report", "redist", "setup", "install", "helper", "launcher", "updater", "vc_", "dxsetup", "easyanticheat", "battleye", "uninstall",
];

/// The game's program inside an install folder: a program named like the
/// game, else the biggest one that isn't an installer or crash reporter.
fn program_in(dir: &Path, game: &str) -> Option<PathBuf> {
    let want = game_key(game);
    let known = crate::games::known_programs(game);
    let mut found: Vec<(PathBuf, u64, bool)> = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() {
                if depth < 4 && !name.starts_with('_') && !["redist", "redistributables", "directx", "commonredist", "engine"].contains(&name.as_str()) {
                    stack.push((p, depth + 1));
                }
            } else if name.ends_with(".exe") && !NOT_THE_GAME.iter().any(|w| name.contains(w)) {
                let raw = name.trim_end_matches(".exe");
                let stem = game_key(raw);
                let named = known.contains(&raw) || (!stem.is_empty() && (want.starts_with(&stem) || stem.starts_with(&want)));
                found.push((p.clone(), e.metadata().map(|m| m.len()).unwrap_or(0), named));
            }
        }
    }
    found.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    found.into_iter().next().map(|f| f.0)
}

fn steam_libraries() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
        let vdf = Path::new(root).join(r"steamapps\libraryfolders.vdf");
        if let Ok(t) = fs::read_to_string(vdf) {
            for line in t.lines() {
                let parts: Vec<&str> = line.split('"').filter(|s| !s.trim().is_empty()).collect();
                if parts.len() == 2 && parts[0] == "path" {
                    libs.push(PathBuf::from(parts[1].replace(r"\\", r"\")).join("steamapps"));
                }
            }
        }
    }
    libs.sort();
    libs.dedup();
    libs
}

fn find_program(game: &str) -> Option<PathBuf> {
    let key = game_key(game);
    if let Some(p) = EXES.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned()).map(PathBuf::from).filter(|p| p.exists()) {
        return Some(p);
    }
    // Epic names the program outright.
    let epic = PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(r"Epic\EpicGamesLauncher\Data\Manifests");
    for e in fs::read_dir(epic).into_iter().flatten().flatten() {
        let Ok(t) = fs::read_to_string(e.path()) else { continue };
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
        if j["DisplayName"].as_str().map(game_key).as_deref() == Some(key.as_str())
            && let (Some(loc), Some(exe)) = (j["InstallLocation"].as_str(), j["LaunchExecutable"].as_str())
        {
            let p = Path::new(loc).join(exe);
            if p.exists() {
                return Some(p);
            }
        }
    }
    // Steam: the install folder from the game's record, then its program.
    for lib in steam_libraries() {
        for e in fs::read_dir(&lib).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Ok(t) = fs::read_to_string(e.path()) else { continue };
            let value = |k: &str| {
                t.lines().find_map(|l| {
                    let p: Vec<&str> = l.split('"').filter(|s| !s.trim().is_empty()).collect();
                    (p.len() == 2 && p[0] == k).then(|| p[1].to_string())
                })
            };
            // "Hunt: Showdown 1896" is the "Hunt Showdown" folder.
            if let (Some(name), Some(dir)) = (value("name"), value("installdir"))
                && (game_key(&name) == key || (key.len() >= 6 && game_key(&name).starts_with(&key)))
                && let Some(p) = program_in(&lib.join("common").join(dir), &name)
            {
                return Some(p);
            }
        }
    }
    // Battle.net and others put games straight in Program Files: "Overwatch" for "Overwatch 2".
    let base_key = key.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    for pf in [r"C:\Program Files (x86)", r"C:\Program Files"] {
        for e in fs::read_dir(pf).into_iter().flatten().flatten() {
            let k = game_key(&e.file_name().to_string_lossy());
            if (k == key || (base_key.len() >= 5 && k == base_key))
                && let Some(p) = program_in(&e.path(), game)
            {
                return Some(p);
            }
        }
    }
    // Riot: "C:\Riot Games\<Game>".
    for drive in ["C", "D", "E", "F"] {
        let riot = PathBuf::from(format!(r"{drive}:\Riot Games"));
        for e in fs::read_dir(&riot).into_iter().flatten().flatten() {
            if game_key(&e.file_name().to_string_lossy()) == key
                && let Some(p) = program_in(&e.path(), game)
            {
                return Some(p);
            }
        }
    }
    None
}

/// The program's icon as a 64×64 PNG (Windows' own icon for it).
/// Logos are kept this many pixels square.
const LOGO: u32 = 128;
/// A program icon at least this many pixels across looks sharp at the sizes logos are shown.
const SHARP: u32 = 48;

/// The program's icon as a LOGO-sized PNG, cropped to the picture itself (Windows
/// pads a small icon with an empty, faintly shaded margin). Returns how many pixels
/// across the picture really is: a program with only a small icon gives a blurry logo.
fn extract(exe: &Path, out: &Path) -> Result<u32, String> {
    let (px, n) = shell_icon(exe)?;
    let (cropped, side) = crop_to_picture(&px, n);
    save_png(&cropped, LOGO, out)?;
    Ok(side)
}

/// Pixels with less cover than this are the see-through frame Windows draws around a
/// small icon, not picture (icons are solid where they're drawn; the frame never is).
const SOLID: u8 = 200;

/// The square around what's drawn (alpha ≥ SOLID) in an n×n RGBA picture, scaled to
/// LOGO pixels; and how many source pixels across that square was.
fn crop_to_picture(px: &[u8], n: u32) -> (Vec<u8>, u32) {
    let (mut x0, mut y0, mut x1, mut y1) = (n, n, 0u32, 0u32);
    for y in 0..n {
        for x in 0..n {
            if px[((y * n + x) * 4 + 3) as usize] >= SOLID {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 || y1 < y0 {
        return (scale(px, n, 0, 0, n, LOGO), n);
    }
    let side = (x1 - x0 + 1).max(y1 - y0 + 1);
    // Centred on the picture, inside the image.
    let cx = (x0 + x1 + 1) / 2;
    let cy = (y0 + y1 + 1) / 2;
    let left = cx.saturating_sub(side / 2).min(n - side);
    let top = cy.saturating_sub(side / 2).min(n - side);
    (scale(px, n, left, top, side, LOGO), side)
}

/// The side×side square at (left, top) of an n-wide RGBA picture, resized to `to`×`to`
/// (bilinear, with premultiplied alpha so edges don't go dark).
fn scale(px: &[u8], n: u32, left: u32, top: u32, side: u32, to: u32) -> Vec<u8> {
    // Sampling stays inside the square: nothing from beyond its edge bleeds in.
    let (right, bottom) = ((left + side).min(n) - 1, (top + side).min(n) - 1);
    let at = |x: u32, y: u32| -> [f32; 4] {
        let i = ((y.clamp(top, bottom) * n + x.clamp(left, right)) * 4) as usize;
        let a = px[i + 3] as f32 / 255.0;
        [px[i] as f32 * a, px[i + 1] as f32 * a, px[i + 2] as f32 * a, a]
    };
    let mut out = vec![0u8; (to * to * 4) as usize];
    let ratio = side as f32 / to as f32;
    for oy in 0..to {
        for ox in 0..to {
            let sx = (left as f32 + (ox as f32 + 0.5) * ratio - 0.5).max(0.0);
            let sy = (top as f32 + (oy as f32 + 0.5) * ratio - 0.5).max(0.0);
            let (x, y) = (sx.floor() as u32, sy.floor() as u32);
            let (fx, fy) = (sx - x as f32, sy - y as f32);
            let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
            let mut v = [0f32; 4];
            for k in 0..4 {
                v[k] = a[k] * (1.0 - fx) * (1.0 - fy) + b[k] * fx * (1.0 - fy) + c[k] * (1.0 - fx) * fy + d[k] * fx * fy;
            }
            let o = ((oy * to + ox) * 4) as usize;
            let alpha = v[3].clamp(0.0, 1.0);
            for k in 0..3 {
                out[o + k] = if alpha > 0.0 { (v[k] / alpha).round().clamp(0.0, 255.0) as u8 } else { 0 };
            }
            out[o + 3] = (alpha * 255.0).round() as u8;
        }
    }
    out
}

/// A square PNG as RGBA pixels (any colour type), with its side; None if it isn't square.
fn decode_square_png(bytes: &[u8]) -> Option<(Vec<u8>, u32)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    if w != h || w == 0 || w > 1024 {
        return None;
    }
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    (rgba.len() == (w * h * 4) as usize).then_some((rgba, w))
}

fn save_png(rgba: &[u8], side: u32, out: &Path) -> Result<(), String> {
    let tmp = out.with_extension("part");
    {
        let file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), side, side);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// The program's icon from Windows, 256×256 RGBA (the biggest size an icon comes in).
fn shell_icon(exe: &Path) -> Result<(Vec<u8>, u32), String> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY};
    use windows::core::HSTRING;
    const N: i32 = 256;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(exe.as_os_str()), None).map_err(|e| e.to_string())?;
        let bmp = f.GetImage(SIZE { cx: N, cy: N }, SIIGBF_ICONONLY).map_err(|e| e.to_string())?;
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = N;
        info.bmiHeader.biHeight = -N; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut px = vec![0u8; (N * N * 4) as usize];
        let dc = GetDC(None);
        let rows = GetDIBits(dc, bmp, 0, N as u32, Some(px.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(bmp.into());
        if rows == 0 {
            return Err("couldn't read the icon".into());
        }
        // BGRA → RGBA
        for p in px.chunks_mut(4) {
            p.swap(0, 2);
        }
        Ok((px, N as u32))
    }
}

/// Folder names that are never a game (and never looked up online).
fn not_a_game(key: &str) -> bool {
    crate::game_detect::UNSORTED.contains(&key) || ["edits", "edit", "exports", "montages", "trimmed", "highlights", "funny"].contains(&key)
}

/// Is a Steam title the same game as a folder? Same name, or one that only
/// adds an edition at the end ("Hunt: Showdown 1896" for "Hunt Showdown",
/// "Resident Evil 3" for "Resident Evil 3 Remake").
fn same_game(folder_key: &str, steam_name: &str) -> bool {
    let k = game_key(steam_name);
    // Only a number or an edition may differ: "Hunt: Showdown 1896" for "Hunt Showdown",
    // "Overwatch" for "Overwatch 2", but not "Minecraft Dungeons" for "Minecraft".
    let edition = |tail: &str| {
        tail.chars().all(|c| c.is_ascii_digit())
            // "Sea of Thieves: 2026 Edition"
            || tail.strip_suffix("edition").is_some_and(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()))
            || ["x", "remake", "remastered", "hd", "goty", "definitiveedition", "enhancededition", "gameoftheyearedition", "classic", "reloaded"].contains(&tail)
            // "Call of Duty Ghosts Multiplayer" is "Call of Duty: Ghosts" (the part played, not another game).
            || ["multiplayer", "singleplayer", "campaign", "zombies"].contains(&tail)
    };
    k == folder_key
        || (folder_key.len() >= 6 && k.strip_prefix(folder_key).is_some_and(edition))
        || (k.len() >= 8 && folder_key.strip_prefix(k.as_str()).is_some_and(edition))
}

/// Games whose Steam name changed, or that the store search no longer lists.
const STEAM_ALIASES: [(&str, u64); 11] = [
    ("tarkov", 3932890),
    ("eft", 3932890),
    ("hitman3", 1659040),
    ("hitman", 1659040),
    ("rocketleague", 252950),
    ("counterstrikeglobaloffensive", 730),
    ("csgo", 730),
    ("cs2", 730),
    ("pubg", 578080),
    ("gtav", 271590),
    ("grandtheftautov", 271590),
];

/// The Steam app for a game: a known alias, this PC's Steam records, else the store search.
fn steam_app(game: &str, client: &reqwest::blocking::Client) -> Option<u64> {
    let key = game_key(game);
    if let Some((_, id)) = STEAM_ALIASES.iter().find(|(k, _)| *k == key) {
        return Some(*id);
    }
    for lib in steam_libraries() {
        for e in fs::read_dir(&lib).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            let Some(id) = n.strip_prefix("appmanifest_").and_then(|r| r.strip_suffix(".acf")).and_then(|r| r.parse::<u64>().ok()) else { continue };
            let Ok(t) = fs::read_to_string(e.path()) else { continue };
            let name = t.lines().find_map(|l| {
                let p: Vec<&str> = l.split('"').filter(|s| !s.trim().is_empty()).collect();
                (p.len() == 2 && p[0] == "name").then(|| p[1].to_string())
            });
            if name.is_some_and(|n| same_game(&key, &n)) {
                return Some(id);
            }
        }
    }
    // "Resident Evil 3 Remake" finds nothing, "Resident Evil 3" does: drop up to two words from the end.
    // A word that's only punctuation ("Empyrion - Galactic Survival") makes Steam find nothing.
    let words: Vec<&str> = game.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).collect();
    for keep in (words.len().saturating_sub(2).max(1)..=words.len()).rev() {
        let term: String = words[..keep]
            .join(" ")
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        let Some(j) = client
            .get(format!("https://store.steampowered.com/api/storesearch/?term={term}&l=english&cc=US"))
            .send()
            .ok()
            .and_then(|r| r.json::<serde_json::Value>().ok())
        else {
            return None;
        };
        // The closest name wins: an exact match over one with an edition added.
        let best = j["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["type"].as_str() == Some("app"))
            .filter_map(|i| Some((i["id"].as_u64()?, i["name"].as_str()?.to_string())))
            .filter(|(_, n)| same_game(&key, n))
            .min_by_key(|(_, n)| game_key(n).len().abs_diff(key.len()))
            .map(|(id, _)| id);
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Games on Steam whose name matches what you're typing (for naming a new game
/// folder): only the typed text is sent. Soundtracks, DLC packs and bundles are left out.
pub fn search_steam(term: &str) -> Result<Vec<String>, String> {
    let term = term.trim();
    if term.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .user_agent("Hindsight")
        .build()
        .map_err(|e| e.to_string())?;
    let q: String = term.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let j: serde_json::Value = client
        .get(format!("https://store.steampowered.com/api/storesearch/?term={q}&l=english&cc=US"))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;
    const EXTRAS: [&str; 15] = [
        "soundtrack", "dlc", "bundle", "pack", "season pass", "artbook", "upgrade", "expansion", "supporter", "currency", "offer", "coins", "points",
        "in-game", "ingame",
    ];
    // Store items aren't games either, nor names dressed up with emoji or symbols.
    let symbols = |s: &str| s.chars().any(|c| c as u32 >= 0x2190 && !c.is_alphanumeric());
    let mut out: Vec<String> = Vec::new();
    for name in j["items"].as_array().into_iter().flatten().filter(|i| i["type"].as_str() == Some("app")).filter_map(|i| i["name"].as_str()) {
        let lower = name.to_lowercase();
        // "Tom Clancy's Rainbow Six® Siege X": no trademark signs in a folder name.
        if symbols(name) {
            continue;
        }
        let clean = name.replace(['®', '™', '©'], "").replace('’', "'").split_whitespace().collect::<Vec<_>>().join(" ");
        // "Sea of Thieves: 2026 Edition" is the "Sea of Thieves" folder.
        let clean = match clean.split_once(": ") {
            Some((head, _)) if same_game(&game_key(head), &clean) => head.to_string(),
            _ => clean,
        };
        if !EXTRAS.iter().any(|w| lower.contains(w)) && !clean.is_empty() && !out.iter().any(|o| game_key(o) == game_key(&clean)) {
            out.push(clean);
        }
    }
    out.truncate(6);
    Ok(out)
}

/// The game's official Steam icon, downloaded into `out`.
fn steam_icon(game: &str, out: &Path) -> Result<(), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Hindsight")
        .build()
        .map_err(|e| e.to_string())?;
    let id = steam_app(game, &client).ok_or("not on Steam")?;
    let j: serde_json::Value = client
        .get(format!("https://api.steampowered.com/ICommunityService/GetApps/v1/?appids%5B0%5D={id}"))
        .send()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;
    let hash = j["response"]["apps"][0]["icon"].as_str().filter(|h| h.chars().all(|c| c.is_ascii_hexdigit()) && !h.is_empty()).ok_or("no icon")?;
    // Newer icons are only at the first address, older ones at either.
    let mut bytes = None;
    for host in ["https://shared.akamai.steamstatic.com/community_assets/images/apps", "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps"] {
        if let Ok(b) = client.get(format!("{host}/{id}/{hash}.jpg")).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()) {
            bytes = Some(b);
            break;
        }
    }
    let bytes = bytes.ok_or("the icon wouldn't download")?;
    // A real JPEG, not an error page.
    if bytes.len() < 200 || bytes[..2] != [0xff, 0xd8] {
        return Err("not an image".into());
    }
    let tmp = out.with_extension("part");
    fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// Games whose Steam page now belongs to their successor: their own icon, still on
/// Steam's servers (game key → icon). CS:GO's page became Counter-Strike 2's in 2023.
const FIXED_ICONS: [(&str, &str); 2] = [
    ("counterstrikeglobaloffensive", "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/730/69f7ebe2735c366c65c0b33dae00e12dc40edbe4.jpg"),
    ("csgo", "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/730/69f7ebe2735c366c65c0b33dae00e12dc40edbe4.jpg"),
];

fn download_jpeg(url: &str, out: &Path) -> Result<(), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(15)).user_agent("Hindsight").build().map_err(|e| e.to_string())?;
    let bytes = client.get(url).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()).map_err(|e| e.to_string())?;
    if bytes.len() < 200 || bytes[..2] != [0xff, 0xd8] {
        return Err("not an image".into());
    }
    let tmp = out.with_extension("part");
    fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())
}

const SITE_EXTS: [&str; 4] = ["png", "jpg", "ico", "webp"];
/// Wikimedia asks automated clients to name themselves and where to find them.
const WIKI_AGENT: &str = "Hindsight/0.1 (https://github.com/aattiaibrahim/hindsight)";

/// A game's official website, from Wikidata (Wikipedia's free database): the video
/// games named like the folder, and the "official website" of the one whose name
/// (or another name it goes by) is the same game. Only the game's name is sent.
fn wikidata_sites(game: &str) -> Result<Vec<String>, String> {
    let key = game_key(game);
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(15)).user_agent(WIKI_AGENT).build().map_err(|e| e.to_string())?;
    let enc = |s: &str| -> String { s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect() };
    let get = |url: String| -> Result<serde_json::Value, String> {
        client.get(url).send().and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?.json().map_err(|e| e.to_string())
    };
    // Full-text search among video games only (Q7889): it finds "Escape from Tarkov: Arena"
    // for "Escape From Tarkov Arena", which a plain name search doesn't.
    let found = get(format!(
        "https://www.wikidata.org/w/api.php?action=query&list=search&srsearch={}&srlimit=6&format=json",
        enc(&format!("haswbstatement:P31=Q7889 {game}"))
    ))?;
    let ids: Vec<String> = found["query"]["search"].as_array().into_iter().flatten().filter_map(|h| h["title"].as_str().map(str::to_string)).collect();
    if ids.is_empty() {
        return Err("not on Wikidata".into());
    }
    let items = get(format!(
        "https://www.wikidata.org/w/api.php?action=wbgetentities&ids={}&props=claims%7Clabels%7Caliases&languages=en%7Cmul&format=json",
        ids.join("%7C")
    ))?;
    for id in &ids {
        let item = &items["entities"][id.as_str()];
        // English names, and the ones for every language ("mul", where Fortnite keeps its name).
        let names = ["en", "mul"].into_iter().flat_map(|l| {
            std::iter::once(item["labels"][l]["value"].as_str())
                .chain(item["aliases"][l].as_array().into_iter().flatten().map(|a| a["value"].as_str()))
                .flatten()
                .collect::<Vec<_>>()
        });
        if !names.into_iter().any(|n| same_game(&key, n)) {
            continue;
        }
        let sites: Vec<String> = item["claims"]["P856"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["mainsnak"]["datavalue"]["value"].as_str())
            .filter(|s| s.starts_with("https://") || s.starts_with("http://"))
            .map(str::to_string)
            .collect();
        if !sites.is_empty() {
            return Ok(sites);
        }
    }
    Err("no official website on Wikidata".into())
}

/// The value of `name="…"` (or '…') in an HTML tag, lowercase name.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&format!("{name}=")) {
        let at = from + i;
        from = at + name.len() + 1;
        // A whole attribute name, not the end of another ("data-href=").
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = &tag[from..];
        let q = rest.chars().next()?;
        let value = if q == '"' || q == '\'' { rest[1..].split(q).next()? } else { rest.split(|c: char| c.is_whitespace() || c == '>').next()? };
        return Some(value.replace("&amp;", "&"));
    }
    None
}

/// Icon addresses on a web page, best first: Apple touch icons and big PNGs before
/// small ones and the favicon.
fn page_icons(html: &str, page: &str) -> Vec<String> {
    let origin = page.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    let absolute = |href: &str| {
        if href.starts_with("http://") || href.starts_with("https://") {
            href.to_string()
        } else if let Some(rest) = href.strip_prefix("//") {
            format!("https://{rest}")
        } else {
            format!("{origin}/{}", href.trim_start_matches('/'))
        }
    };
    let mut found: Vec<(u32, String)> = Vec::new();
    for part in html.split("<link").skip(1).chain(html.split("<LINK").skip(1)) {
        let tag = part.split('>').next().unwrap_or("");
        let Some(rel) = attr(tag, "rel").map(|r| r.to_lowercase()) else { continue };
        let Some(href) = attr(tag, "href") else { continue };
        if !rel.contains("icon") || rel.contains("mask") || href.to_lowercase().contains(".svg") {
            continue;
        }
        let size = attr(tag, "sizes").and_then(|s| s.split(['x', 'X']).next().and_then(|n| n.trim().parse::<u32>().ok()));
        let score = if rel.contains("apple-touch") { size.unwrap_or(180).max(150) } else { size.unwrap_or(if href.to_lowercase().contains(".ico") { 32 } else { 64 }) };
        found.push((score, absolute(&href)));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<String> = found.into_iter().map(|(_, u)| u).collect();
    out.push(format!("{origin}/apple-touch-icon.png"));
    out.push(format!("{origin}/favicon.ico"));
    out.dedup();
    out
}

/// What kind of picture these bytes are (by their first bytes), if one the app can show.
fn picture_ext(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if b.starts_with(&[0xff, 0xd8]) {
        Some("jpg")
    } else if b.starts_with(&[0, 0, 1, 0]) {
        Some("ico")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// The icon from the first of these websites that has one: each page as given, then
/// its home page (a deep link can redirect somewhere that fails).
fn icon_from_sites(sites: &[String], dir: &Path, key: &str) -> Result<PathBuf, String> {
    let mut tried: Vec<String> = Vec::new();
    let mut last = String::from("no website");
    // A small icon is kept only if no other page has a sharp one.
    let mut fallback: Option<PathBuf> = None;
    for site in sites.iter().take(3) {
        let home = format!("{}/", site.splitn(4, '/').take(3).collect::<Vec<_>>().join("/"));
        for page in [site.clone(), home] {
            if tried.contains(&page) {
                continue;
            }
            tried.push(page.clone());
            match site_icon(&page, dir, key) {
                Ok((p, true)) => return Ok(p),
                Ok((p, false)) => {
                    // Kept aside under its own name, so a later page can't overwrite it half-way.
                    let aside = p.with_extension(format!("{}.small", p.extension().and_then(|e| e.to_str()).unwrap_or("")));
                    if fallback.is_none() && fs::rename(&p, &aside).is_ok() {
                        fallback = Some(aside);
                    } else {
                        let _ = fs::remove_file(&p);
                    }
                }
                Err(e) => last = format!("{page}: {e}"),
            }
        }
    }
    if let Some(aside) = fallback {
        // "<key>.site.ico.small" back to "<key>.site.ico".
        let back = aside.with_extension("");
        fs::rename(&aside, &back).map_err(|e| e.to_string())?;
        return Ok(back);
    }
    Err(last)
}

/// The game's icon from its own website, saved as `<key>.site.<ext>`.
/// The page's icon, saved for `key`; and whether it's sharp (else it was the only one).
fn site_icon(page: &str, dir: &Path, key: &str) -> Result<(PathBuf, bool), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Hindsight")
        .build()
        .map_err(|e| e.to_string())?;
    let html = client.get(page).send().and_then(|r| r.error_for_status()).and_then(|r| r.text()).map_err(|e| e.to_string())?;
    // The first sharp icon wins; a small one (a 16-pixel favicon) only if there's nothing better.
    let mut small: Option<(Vec<u8>, &'static str)> = None;
    for url in page_icons(&html, page).into_iter().take(6) {
        let Ok(bytes) = client.get(&url).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()) else { continue };
        if bytes.len() < 100 || bytes.len() > 2_000_000 {
            continue;
        }
        let Some(ext) = picture_ext(&bytes) else { continue };
        if picture_side(&bytes, ext) >= SHARP {
            return save_site_icon(&bytes, ext, dir, key).map(|p| (p, true));
        }
        if small.is_none() {
            small = Some((bytes.to_vec(), ext));
        }
    }
    match small {
        Some((bytes, ext)) => save_site_icon(&bytes, ext, dir, key).map(|p| (p, false)),
        None => Err("no icon on the page".into()),
    }
}

/// How many pixels across a downloaded icon's picture is (a PNG's, cropped; an ICO's
/// biggest size). Other kinds aren't measured and count as sharp.
fn picture_side(bytes: &[u8], ext: &str) -> u32 {
    match ext {
        "png" => decode_square_png(bytes).map(|(rgba, n)| crop_to_picture(&rgba, n).1).unwrap_or(0),
        "ico" => {
            let count = bytes.get(4..6).map(|c| u16::from_le_bytes([c[0], c[1]]) as usize).unwrap_or(0);
            (0..count.min(64)).filter_map(|i| bytes.get(6 + i * 16)).map(|&w| if w == 0 { 256 } else { w as u32 }).max().unwrap_or(0)
        }
        _ => SHARP,
    }
}

/// Saved as `<key>.site.<ext>`; a square PNG cropped to its picture, like program icons.
fn save_site_icon(bytes: &[u8], ext: &str, dir: &Path, key: &str) -> Result<PathBuf, String> {
    let out = dir.join(format!("{key}.site.{ext}"));
    if ext == "png"
        && let Some((rgba, n)) = decode_square_png(bytes)
    {
        let (cropped, _) = crop_to_picture(&rgba, n);
        save_png(&cropped, LOGO, &out)?;
        return Ok(out);
    }
    let tmp = out.with_extension("part");
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn tried_recently(marker: &Path, secs: u64) -> bool {
    fs::metadata(marker)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|e| e.as_secs() < secs).unwrap_or(true))
        .unwrap_or(false)
}

const CUSTOM_EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn custom_logo(dir: &Path, key: &str) -> Option<PathBuf> {
    CUSTOM_EXTS.iter().map(|e| dir.join(format!("{key}.custom.{e}"))).find(|p| p.exists())
}

/// Use a picture you chose as a game's logo (`None` goes back to the automatic one).
pub fn set_logo(game: &str, picture: Option<&Path>) -> Result<(), String> {
    let dir = DIR.lock().unwrap().clone().ok_or("not ready")?;
    let key = game_key(game);
    if key.is_empty() {
        return Err("That folder has no name to keep a logo for.".into());
    }
    for e in CUSTOM_EXTS {
        let _ = fs::remove_file(dir.join(format!("{key}.custom.{e}")));
    }
    let Some(src) = picture else { return Ok(()) };
    let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).filter(|e| CUSTOM_EXTS.contains(&e.as_str())).ok_or("Pick a PNG, JPG or WebP picture.")?;
    if fs::metadata(src).map(|m| m.len()).unwrap_or(0) > 8 * 1024 * 1024 {
        return Err("That picture is over 8 MB. Pick a smaller one.".into());
    }
    fs::copy(src, dir.join(format!("{key}.custom.{ext}"))).map(|_| ()).map_err(|e| e.to_string())
}

/// The icon for a game folder, made once and kept. None = nothing found
/// (checked again a day later, in case the game gets installed or played).
/// `online`: look the official icon up on Steam first. A logo you picked
/// yourself always wins.
pub fn icon(game: &str, online: bool) -> Option<PathBuf> {
    let dir = DIR.lock().unwrap().clone()?;
    let key = game_key(game);
    if key.is_empty() {
        return None;
    }
    if let Some(p) = custom_logo(&dir, &key) {
        return Some(p);
    }
    // A game whose Steam page became another game's keeps its own old icon.
    if online && let Some(url) = FIXED_ICONS.iter().find(|(k, _)| *k == key).map(|(_, u)| *u) {
        let fixed = dir.join(format!("{key}.fixed.jpg"));
        if fixed.exists() {
            return Some(fixed);
        }
        let miss = dir.join(format!("{key}.fixed-none"));
        if !crate::games::game_running() && !tried_recently(&miss, 86_400) {
            match download_jpeg(url, &fixed) {
                Ok(()) => return Some(fixed),
                Err(e) => {
                    crate::log!("game logo for {game}: {e}");
                    let _ = fs::write(&miss, b"");
                }
            }
        }
    }
    if online && !not_a_game(&key) {
        // ".steam2": looked up with the stricter name match (older lookups could
        // give "Minecraft" the logo of "Minecraft Dungeons").
        let steam = dir.join(format!("{key}.steam2.jpg"));
        if steam.exists() {
            return Some(steam);
        }
        // Never while a game is running; a miss is asked again after a week.
        // "steam5": asked again after "Sea of Thieves: 2026 Edition" and "… Multiplayer" started to match.
        let miss = dir.join(format!("{key}.steam5-none"));
        if !crate::games::game_running() && !tried_recently(&miss, 7 * 86_400) {
            // "SpeciesUnknown-Win64-Shipping" is looked up as "Species Unknown".
            let title = crate::games::tidy_program_name(game).unwrap_or_else(|| game.to_string());
            match steam_icon(&title, &steam) {
                Ok(()) => return Some(steam),
                Err(e) => {
                    crate::log!("game logo for {game}: {e}");
                    let _ = fs::write(&miss, b"");
                }
            }
        }
    }
    // The game's own icon from its program, cropped to the picture (".logo": older
    // versions kept it padded, as a small square in a dark frame).
    let png = dir.join(format!("{key}.logo.png"));
    // Marks a program icon too small to be sharp: the website's is used if there is one.
    let small = dir.join(format!("{key}.logo-small"));
    if png.exists() && !small.exists() {
        return Some(png);
    }
    if !png.exists() {
        let miss = dir.join(format!("{key}.none"));
        // A remembered program (a game just played) is always worth trying again.
        let remembered = EXES.lock().unwrap().as_ref().is_some_and(|m| m.contains_key(&key));
        if !tried_recently(&miss, 86_400) || remembered {
            match find_program(game).map(|exe| extract(&exe, &png)) {
                Some(Ok(side)) => {
                    let _ = fs::remove_file(&miss);
                    if side >= SHARP {
                        let _ = fs::remove_file(&small);
                        return Some(png);
                    }
                    let _ = fs::write(&small, b"");
                }
                _ => {
                    let _ = fs::remove_file(&png);
                    let _ = fs::write(&miss, b"");
                }
            }
        }
    }
    // Not on Steam, and not installed or only with a small icon (Minecraft, Roblox, Star
    // Citizen, VALORANT…): the icon from the game's own website, found through Wikidata.
    // A miss is asked again after a week, never while you play.
    if online && !not_a_game(&key) {
        if let Some(p) = SITE_EXTS.iter().map(|e| dir.join(format!("{key}.site.{e}"))).find(|p| p.exists()) {
            return Some(p);
        }
        let miss = dir.join(format!("{key}.site2-none"));
        if !crate::games::game_running() && !tried_recently(&miss, 7 * 86_400) {
            let title = crate::games::tidy_program_name(game).unwrap_or_else(|| game.to_string());
            match wikidata_sites(&title).and_then(|sites| icon_from_sites(&sites, &dir, &key)) {
                Ok(p) => return Some(p),
                Err(e) => {
                    crate::log!("game logo for {game} from its website: {e}");
                    let _ = fs::write(&miss, b"");
                }
            }
        }
    }
    // A small program icon is still better than none.
    png.exists().then_some(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_names_match_only_the_same_game() {
        assert!(same_game("huntshowdown", "Hunt: Showdown 1896"));
        assert!(same_game("overwatch2", "Overwatch®"));
        assert!(same_game("tomclancysrainbowsixsiege", "Tom Clancy's Rainbow Six® Siege X"));
        assert!(!same_game("minecraft", "Minecraft Dungeons"));
        assert!(!same_game("render", "Render Voice VR"));
        assert!(same_game("residentevil3remake", "Resident Evil 3"));
        assert!(same_game("counterstrike2", "Counter-Strike 2"));
        assert!(same_game("seaofthieves", "Sea of Thieves: 2026 Edition"));
        assert!(same_game("callofdutyghostsmultiplayer", "Call of Duty®: Ghosts"));
        assert!(!same_game("callofdutyghosts", "Call of Duty®: Ghosts - Onslaught"));
        assert!(!same_game("seaofthieves", "Sea of Thieves: 2026 Premium Bundle"));
        assert!(!same_game("edit", "Edith Finch"));
        assert!(!same_game("funnynotcool", "Funny"));
        assert!(!same_game("residentevilrequiem", "Resident Evil"));
        assert!(not_a_game("desktop") && not_a_game("edits") && !not_a_game("valorant"));
    }

    #[test]
    fn a_small_icon_is_cropped_out_of_its_shaded_margin() {
        // 64×64: a faint shaded frame (what Windows adds) around a solid red 32×32 icon.
        let n = 64u32;
        let mut px = vec![0u8; (n * n * 4) as usize];
        for y in 0..n {
            for x in 0..n {
                let i = ((y * n + x) * 4) as usize;
                let inside = (16..48).contains(&x) && (16..48).contains(&y);
                px[i..i + 4].copy_from_slice(&if inside { [255, 70, 84, 255] } else { [85, 85, 85, 30] });
            }
        }
        let (out, side) = crop_to_picture(&px, n);
        assert_eq!(side, 32, "the picture itself, not the frame");
        assert_eq!(out.len(), (LOGO * LOGO * 4) as usize);
        // Corners are the icon's red now, not the frame.
        for i in [0, ((LOGO * LOGO - 1) * 4) as usize] {
            assert_eq!(&out[i..i + 4], &[255, 70, 84, 255]);
        }
        assert!(side < SHARP, "a 32-pixel icon is too small to be sharp");
    }

    #[test]
    fn a_websites_best_icon_is_found() {
        let html = r#"<head><link rel="icon" href="/favicon.ico"><LINK rel="icon" type="image/png" sizes="192x192" href="/img/icon-192.png"/>
            <link rel="apple-touch-icon" href="https://cdn.example.com/touch.png?a=1&amp;b=2"><link rel="mask-icon" href="/m.svg">
            <link data-href="x" rel="stylesheet" href="/s.css"></head>"#;
        let got = page_icons(html, "https://www.example.com/en-us/");
        assert_eq!(got[0], "https://www.example.com/img/icon-192.png");
        assert_eq!(got[1], "https://cdn.example.com/touch.png?a=1&b=2");
        assert!(got.contains(&"https://www.example.com/favicon.ico".to_string()));
        assert!(!got.iter().any(|u| u.ends_with(".svg") || u.ends_with(".css")));
        assert_eq!(picture_ext(&[0x89, b'P', b'N', b'G', 0]), Some("png"));
        assert_eq!(picture_ext(b"<html>"), None);
    }

    #[test]
    fn picks_the_game_program_over_helpers() {
        let d = std::env::temp_dir().join(format!("rewind-icons-{}", std::process::id()));
        let bin = d.join(r"game\bin\win64");
        fs::create_dir_all(&bin).unwrap();
        fs::write(d.join("UnityCrashHandler64.exe"), vec![0u8; 9000]).unwrap();
        fs::write(d.join("unins000.exe"), vec![0u8; 9000]).unwrap();
        fs::write(bin.join("cs2.exe"), vec![0u8; 100]).unwrap();
        fs::write(bin.join("other.exe"), vec![0u8; 500]).unwrap();
        assert_eq!(program_in(&d, "Some Game").unwrap().file_name().unwrap(), "other.exe", "no name match: biggest");
        assert_eq!(program_in(&d, "Counter-Strike 2").unwrap().file_name().unwrap(), "cs2.exe", "a known program name wins");
        fs::write(bin.join("SomeGame.exe"), vec![0u8; 10]).unwrap();
        assert_eq!(program_in(&d, "Some Game").unwrap().file_name().unwrap(), "SomeGame.exe");
        let _ = fs::remove_dir_all(d);
    }

    /// Makes icons for real games on this PC: cargo test live_game_icons -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_game_icons() {
        let dir = std::env::temp_dir().join("rewind-live-icons");
        init(dir.clone());
        for g in ["VALORANT", "League of Legends", "Fortnite", "Moon Sweepers", "Minecraft", "Roblox", "Star Citizen", "Escape From Tarkov Arena", "aim", "BenQ", "Call of Duty Ghosts Multiplayer", "Counter-strike Global Offensive", "Sea of Thieves", "Tarkov", "Payday The Heist", "Battlefield 6", "Minecraft", "Render", "Hitman 3", "Rocket League", "Apex Legends", "SpeciesUnknown-Win64-Shipping", "Species Unknown", "Escape From Tarkov", "Resident Evil 3 Remake", "Edits", "funny not cool", "VALORANT", "League of Legends", "Counter-strike 2", "Marvel Rivals", "Overwatch 2", "Tom Clancys Rainbow Six Siege", "Hunt Showdown", "Payday The Heist", "Resident Evil Requiem", "The Outlast Trials", "Empyrion - Galactic Survival", "Desktop"] {
            println!("{g}: {:?} -> {:?}", find_program(g), icon(g, true));
        }
    }
}
