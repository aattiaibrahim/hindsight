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
fn extract(exe: &Path, out: &Path) -> Result<(), String> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY};
    use windows::core::HSTRING;
    const N: i32 = 64;
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
        let file = fs::File::create(out).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), N as u32, N as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(&px)).map_err(|e| e.to_string())
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
            || ["x", "remake", "remastered", "hd", "goty", "definitiveedition", "enhancededition", "gameoftheyearedition", "classic", "reloaded"].contains(&tail)
    };
    k == folder_key
        || (folder_key.len() >= 6 && k.strip_prefix(folder_key).is_some_and(edition))
        || (k.len() >= 8 && folder_key.strip_prefix(k.as_str()).is_some_and(edition))
}

/// Games whose Steam name changed, or that the store search no longer lists.
const STEAM_ALIASES: [(&str, u64); 9] = [
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
    if online && !not_a_game(&key) {
        // ".steam2": looked up with the stricter name match (older lookups could
        // give "Minecraft" the logo of "Minecraft Dungeons").
        let steam = dir.join(format!("{key}.steam2.jpg"));
        if steam.exists() {
            return Some(steam);
        }
        // Never while a game is running; a miss is asked again after a week.
        // "steam3": asked again after the search learned to skip punctuation words.
        let miss = dir.join(format!("{key}.steam3-none"));
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
    let png = dir.join(format!("{key}.png"));
    if png.exists() {
        return Some(png);
    }
    let miss = dir.join(format!("{key}.none"));
    let recent_miss = tried_recently(&miss, 86_400);
    // A remembered program (a game just played) is always worth trying again.
    let remembered = EXES.lock().unwrap().as_ref().is_some_and(|m| m.contains_key(&key));
    if recent_miss && !remembered {
        return None;
    }
    match find_program(game).map(|exe| extract(&exe, &png)) {
        Some(Ok(())) => {
            let _ = fs::remove_file(&miss);
            Some(png)
        }
        _ => {
            let _ = fs::remove_file(&png);
            let _ = fs::write(&miss, b"");
            None
        }
    }
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
        assert!(!same_game("edit", "Edith Finch"));
        assert!(!same_game("funnynotcool", "Funny"));
        assert!(!same_game("residentevilrequiem", "Resident Evil"));
        assert!(not_a_game("desktop") && not_a_game("edits") && !not_a_game("valorant"));
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
        for g in ["Payday The Heist", "Battlefield 6", "Minecraft", "Render", "Hitman 3", "Rocket League", "Apex Legends", "SpeciesUnknown-Win64-Shipping", "Species Unknown", "Escape From Tarkov", "Resident Evil 3 Remake", "Edits", "funny not cool", "VALORANT", "League of Legends", "Counter-strike 2", "Marvel Rivals", "Overwatch 2", "Tom Clancys Rainbow Six Siege", "Hunt Showdown", "Payday The Heist", "Resident Evil Requiem", "The Outlast Trials", "Empyrion - Galactic Survival", "Desktop"] {
            println!("{g}: {:?} -> {:?}", find_program(g), icon(g, true));
        }
    }
}
