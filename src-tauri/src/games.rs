//! Is the app in front a game, and what's it called?
//!
//! Used to file clips: a detected game gets its own folder, everything else
//! goes to "Desktop". Detection, in order:
//! 1. Never a game: browsers, chat, launchers, editors, video players…
//! 2. A known game exe (e.g. VALORANT-Win64-Shipping → "VALORANT").
//! 3. Installed in a game library (steamapps\common, Epic Games, Riot Games…),
//!    named after its folder there.
//! 4. Fullscreen or borderless-fullscreen, named from its window title.

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

pub const DESKTOP_FOLDER: &str = "Desktop";

const NOT_GAMES: &[&str] = &[
    "explorer", "hindsight", "rewind", "searchhost", "shellexperiencehost", "startmenuexperiencehost",
    "applicationframehost", "lockapp", "textinputhost", "taskmgr", "systemsettings",
    "chrome", "msedge", "firefox", "opera", "opera_gx", "brave", "vivaldi", "arc", "zen",
    "discord", "discordptb", "discordcanary", "slack", "teams", "ms-teams", "zoom", "telegram", "whatsapp",
    "spotify", "vlc", "mpc-hc64", "mpc-be64", "mpv", "potplayermini64", "wmplayer", "music.ui", "video.ui",
    "steam", "steamwebhelper", "epicgameslauncher", "battle.net", "riotclientservices", "riotclientux",
    "eadesktop", "origin", "upc", "ubisoftconnect", "galaxyclient", "gamingservices", "xboxpcapp",
    "obs64", "medal", "nvidia app", "nvcontainer", "code", "devenv", "notepad", "notepad++",
    "windowsterminal", "powershell", "pwsh", "cmd", "winword", "excel", "powerpnt", "outlook",
    "photoshop", "premiere pro", "afterfx", "resolve", "claude",
];

const KNOWN: &[(&str, &str)] = &[
    ("valorant-win64-shipping", "VALORANT"),
    ("r5apex", "Apex Legends"),
    ("r5apex_dx12", "Apex Legends"),
    ("cs2", "Counter-Strike 2"),
    ("fortniteclient-win64-shipping", "Fortnite"),
    ("league of legends", "League of Legends"),
    ("overwatch", "Overwatch 2"),
    ("rocketleague", "Rocket League"),
    ("minecraft.windows", "Minecraft"),
    ("gta5", "Grand Theft Auto V"),
    ("gta5_enhanced", "Grand Theft Auto V"),
    ("eldenring", "ELDEN RING"),
    ("cod", "Call of Duty"),
    ("destiny2", "Destiny 2"),
    ("dota2", "Dota 2"),
    ("robloxplayerbeta", "Roblox"),
    ("marvel-win64-shipping", "Marvel Rivals"),
    ("rainbowsix", "Rainbow Six Siege"),
    ("tslgame", "PUBG"),
    ("cyberpunk2077", "Cyberpunk 2077"),
    ("witcher3", "The Witcher 3"),
    ("helldivers2", "HELLDIVERS 2"),
    ("discovery", "THE FINALS"),
    ("deadlock", "Deadlock"),
    ("bf2042", "Battlefield 2042"),
    ("rdr2", "Red Dead Redemption 2"),
];

/// Library folders: the game's name is the folder right after the marker.
const LIBRARIES: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\riot games\\",
    "\\xboxgames\\",
    "\\gog galaxy\\games\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
    "\\rockstar games\\",
    "\\battle.net\\games\\",
];

/// The pure decision, separate from Win32 so it can be tested.
pub fn classify(exe_path: &str, window_title: &str, fullscreen: bool) -> Option<String> {
    let path = Path::new(exe_path);
    let stem = path.file_stem()?.to_string_lossy().to_lowercase();
    if NOT_GAMES.contains(&stem.as_str()) {
        return None;
    }
    if let Some((_, title)) = KNOWN.iter().find(|(exe, _)| *exe == stem) {
        return Some(title.to_string());
    }
    let lower = exe_path.to_lowercase();
    for marker in LIBRARIES {
        if let Some(i) = lower.find(marker) {
            let rest = &exe_path[i + marker.len()..];
            if let Some(name) = rest.split(['\\', '/']).next().filter(|n| !n.is_empty() && rest.len() > n.len()) {
                // Riot keeps "VALORANT\live\..." etc.: the first folder is the game.
                return Some(name.to_string());
            }
        }
    }
    if fullscreen {
        let t = window_title.trim();
        if !t.is_empty() && t.chars().count() <= 60 && !looks_like_exe(t) {
            return Some(t.to_string());
        }
        // No usable title: name it after the program, tidied up, keeping its
        // original capitals ("SpeciesUnknown-Win64-Shipping" → "Species Unknown").
        let original = if looks_like_exe(t) {
            t.to_string()
        } else {
            path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        };
        return Some(nice_name(&clean_exe_name(&original.to_lowercase()), &original));
    }
    None
}

/// Known program names (lowercase, no .exe) for a game title: "Counter-Strike 2" → ["cs2"].
pub fn known_programs(title: &str) -> Vec<&'static str> {
    let key = crate::consolidate::game_key(title);
    KNOWN.iter().filter(|(_, t)| crate::consolidate::game_key(t) == key).map(|(e, _)| *e).collect()
}

/// A folder named after a program ("SpeciesUnknown-Win64-Shipping", saved by
/// older versions): the tidy game name to suggest instead ("Species Unknown").
pub fn tidy_program_name(folder: &str) -> Option<String> {
    looks_like_exe(folder).then(|| nice_name(&clean_exe_name(&folder.to_lowercase()), folder)).filter(|n| n != folder && !n.is_empty())
}

/// "SpeciesUnknown-Win64-Shipping", "MyGame_x64": a program name, not a title.
fn looks_like_exe(t: &str) -> bool {
    !t.contains(' ') && ["-win64", "-shipping", "_x64", "_dx12", "_dx11", "-win32"].iter().any(|s| t.to_lowercase().contains(s))
}

/// Recover the capitals and spaces from the original spelling: "SpeciesUnknown" → "Species Unknown".
fn nice_name(cleaned_lower: &str, original: &str) -> String {
    let orig: String = original.chars().take(cleaned_lower.chars().count()).collect();
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in orig.chars() {
        if ch == '_' || ch == '-' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        out.push(ch);
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { cleaned_lower.to_string() } else { out }
}
fn clean_exe_name(stem: &str) -> String {
    let mut name = stem.to_string();
    for suffix in ["-win64-shipping", "-win64", "_x64", "-shipping", "_dx12", "_dx11"] {
        if let Some(s) = name.strip_suffix(suffix) {
            name = s.to_string();
        }
    }
    name
}

/// Make a name safe to use as a Windows folder.
pub fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    let reserved = ["con", "prn", "aux", "nul", "com1", "lpt1"];
    if cleaned.is_empty() || reserved.contains(&cleaned.to_lowercase().as_str()) {
        DESKTOP_FOLDER.into()
    } else {
        cleaned.chars().take(60).collect()
    }
}

/// The game in front right now, if any (store names preferred, see `detect`).
pub fn foreground_game() -> Option<String> {
    let (path, title, fullscreen, _) = foreground_info()?;
    detect(&path, &title, fullscreen)
}

/// `classify`, but a Steam or Epic game gets the name its store shows
/// ("Counter-Strike 2", not the install folder's "Counter-Strike Global Offensive").
pub fn detect(exe_path: &str, title: &str, fullscreen: bool) -> Option<String> {
    let base = classify(exe_path, title, fullscreen)?;
    let known = KNOWN.iter().any(|(_, n)| *n == base);
    Some(if known { base } else { store::name_for(exe_path).unwrap_or(base) })
}

/// The last game seen in front, so a clip saved while you're tabbed out (to
/// Discord, a browser…) still goes to the game you were playing.
struct Seen {
    game: String,
    pid: u32,
    at: std::time::Instant,
}
static LAST: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
/// How long after you leave a game it still counts (while it's still running).
const STICKY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Look at the window in front (cheap: a few Windows calls). Called every couple of seconds.
pub fn observe() {
    if let Some((path, title, fullscreen, pid)) = foreground_info()
        && let Some(game) = detect(&path, &title, fullscreen)
    {
        crate::game_icons::remember(&game, &path);
        *LAST.lock().unwrap() = Some(Seen { game, pid, at: std::time::Instant::now() });
    }
}

fn still_running(pid: u32) -> bool {
    use windows::Win32::System::Threading::GetExitCodeProcess;
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == 259 // STILL_ACTIVE
    }
}

/// The game for a clip saved now: the one in front, or the one you were just
/// playing if it's still running (you alt-tabbed); else "Desktop".
pub fn current_game() -> Option<String> {
    if let Some(g) = foreground_game() {
        return Some(g);
    }
    let last = LAST.lock().unwrap();
    let s = last.as_ref()?;
    (s.at.elapsed() < STICKY && still_running(s.pid)).then(|| s.game.clone())
}

/// The game `game_running` means: the one in front, or the last one played if it's
/// still open. For saying what something is waiting for.
pub fn running_game() -> Option<String> {
    if let Some(g) = foreground_game() {
        return Some(g);
    }
    let last = LAST.lock().unwrap();
    last.as_ref().filter(|s| still_running(s.pid)).map(|s| s.game.clone())
}

/// Is a game being played: in front, or still running after you tabbed out
/// of it (however long ago). For work that should wait until you're done.
pub fn game_running() -> bool {
    if foreground_game().is_some() {
        return true;
    }
    let last = LAST.lock().unwrap();
    last.as_ref().is_some_and(|s| still_running(s.pid))
}

/// Clip folder for right now: the game's name, or "Desktop".
pub fn clip_folder() -> String {
    current_game().map(|g| folder_name(&g)).unwrap_or_else(|| DESKTOP_FOLDER.into())
}

/// Proper game names from the stores' own install records, read once and cached.
mod store {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    struct Cache {
        /// steamapps folder (lowercase) → installdir (lowercase) → name
        steam: HashMap<String, HashMap<String, String>>,
        /// install location (lowercase, no trailing slash) → display name
        epic: Vec<(String, String)>,
        loaded: Option<Instant>,
    }
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

    /// `"key"   "value"` lines of a Steam .acf file.
    pub fn acf_value(text: &str, key: &str) -> Option<String> {
        text.lines().find_map(|l| {
            let mut parts = l.split('"').filter(|s| !s.trim().is_empty());
            (parts.next()? == key).then(|| parts.next().map(str::to_string)).flatten()
        })
    }

    fn steam_names(steamapps: &Path) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for e in std::fs::read_dir(steamapps).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.starts_with("appmanifest_") && n.ends_with(".acf")
                && let Ok(t) = std::fs::read_to_string(e.path())
                && let (Some(dir), Some(name)) = (acf_value(&t, "installdir"), acf_value(&t, "name"))
            {
                m.insert(dir.to_lowercase(), name);
            }
        }
        m
    }

    fn epic_names() -> Vec<(String, String)> {
        let dir = PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(r"Epic\EpicGamesLauncher\Data\Manifests");
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("item")))
            .filter_map(|e| {
                let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
                let loc = j["InstallLocation"].as_str()?.trim_end_matches(['\\', '/']).to_lowercase();
                Some((loc, j["DisplayName"].as_str()?.to_string()))
            })
            .collect()
    }

    pub fn name_for(exe_path: &str) -> Option<String> {
        let lower = exe_path.to_lowercase();
        let mut guard = CACHE.lock().unwrap();
        let c = guard.get_or_insert_with(|| Cache { steam: HashMap::new(), epic: Vec::new(), loaded: None });
        // Re-read at most every 10 minutes (a game installed since then gets picked up).
        if c.loaded.map(|t| t.elapsed() > Duration::from_secs(600)).unwrap_or(true) {
            c.steam.clear();
            c.epic = epic_names();
            c.loaded = Some(Instant::now());
        }
        if let Some(i) = lower.find(r"\steamapps\common\") {
            let steamapps = lower[..i + r"\steamapps".len()].to_string();
            let install = lower[i + r"\steamapps\common\".len()..].split('\\').next()?.to_string();
            if !c.steam.contains_key(&steamapps) {
                let names = steam_names(Path::new(&exe_path[..i + r"\steamapps".len()]));
                c.steam.insert(steamapps.clone(), names);
            }
            if let Some(n) = c.steam.get(&steamapps).and_then(|m| m.get(&install)) {
                return Some(n.clone());
            }
        }
        c.epic.iter().find(|(loc, _)| !loc.is_empty() && lower.starts_with(&format!("{loc}\\"))).map(|(_, n)| n.clone())
    }
}

fn foreground_info() -> Option<(String, String, bool, u32)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        Some((exe_path(hwnd)?, window_title(hwnd), is_fullscreen(hwnd), pid))
    }
}
unsafe fn exe_path(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()? };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    let _ = unsafe { CloseHandle(h) };
    r.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

unsafe fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Covers its whole monitor (exclusive or borderless fullscreen).
unsafe fn is_fullscreen(hwnd: HWND) -> bool {
    let mut r = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return false;
    }
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !unsafe { GetMonitorInfoW(mon, &mut info) }.as_bool() {
        return false;
    }
    let m = info.rcMonitor;
    r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_exe() {
        let p = r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe";
        assert_eq!(classify(p, "VALORANT  ", true).as_deref(), Some("VALORANT"));
    }

    #[test]
    fn steam_library_folder() {
        let p = r"D:\SteamLibrary\steamapps\common\Hades II\Ship\Hades2.exe";
        assert_eq!(classify(p, "Hades II", false).as_deref(), Some("Hades II"));
    }

    #[test]
    fn epic_library_folder() {
        let p = r"C:\Program Files\Epic Games\RocketLeague\Binaries\Win64\SomeGame.exe";
        assert_eq!(classify(p, "", false).as_deref(), Some("RocketLeague"));
    }

    #[test]
    fn browsers_and_apps_are_desktop() {
        assert_eq!(classify(r"C:\Program Files\Google\Chrome\Application\chrome.exe", "YouTube", true), None);
        assert_eq!(classify(r"C:\Windows\explorer.exe", "", true), None);
        assert_eq!(classify(r"C:\Users\x\AppData\Local\Discord\app-1.0\Discord.exe", "Discord", false), None);
        assert_eq!(classify(r"C:\Program Files (x86)\Steam\steam.exe", "Steam", true), None);
    }

    #[test]
    fn unknown_windowed_app_is_desktop() {
        assert_eq!(classify(r"C:\Tools\paint.exe", "Untitled - Paint", false), None);
    }

    #[test]
    fn unknown_fullscreen_app_uses_title() {
        assert_eq!(classify(r"C:\Games\Indie\game.exe", "Celeste", true).as_deref(), Some("Celeste"));
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("Call of Duty: Black Ops 6"), "Call of Duty Black Ops 6");
        assert_eq!(folder_name("  ...  "), "Desktop");
        assert_eq!(folder_name("CON"), "Desktop");
        assert_eq!(folder_name("Halo?"), "Halo");
    }

    #[test]
    fn exe_style_titles_become_readable_names() {
        let p = r"C:\Games\SpeciesUnknown\SpeciesUnknown-Win64-Shipping.exe";
        assert_eq!(classify(p, "SpeciesUnknown-Win64-Shipping", true).as_deref(), Some("Species Unknown"));
        assert_eq!(classify(r"C:\Games\x\cool_game_x64.exe", "", true).as_deref(), Some("cool game"));
    }

    #[test]
    fn steam_manifest_names_are_read() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n\t\"installdir\"\t\t\"Counter-Strike Global Offensive\"\n}";
        assert_eq!(store::acf_value(acf, "name").as_deref(), Some("Counter-Strike 2"));
        assert_eq!(store::acf_value(acf, "installdir").as_deref(), Some("Counter-Strike Global Offensive"));
        let d = std::env::temp_dir().join(format!("rewind-steam-{}", std::process::id()));
        let apps = d.join("steamapps");
        std::fs::create_dir_all(apps.join(r"common\Counter-Strike Global Offensive\game\bin\win64")).unwrap();
        std::fs::write(apps.join("appmanifest_730.acf"), acf).unwrap();
        let exe = apps.join(r"common\Counter-Strike Global Offensive\game\bin\win64\notcs2.exe");
        assert_eq!(detect(&exe.to_string_lossy(), "", false).as_deref(), Some("Counter-Strike 2"));
        let _ = std::fs::remove_dir_all(d);
    }
}