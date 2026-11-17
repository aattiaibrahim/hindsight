//! Share a clip with a link, ShareX-style.
//!
//! 1. Prepare: Discord and most players only play H.264 MP4 inline, so an
//!    AV1/HEVC/MKV clip is turned into one (or squeezed under 10 MB for a
//!    Discord file upload). A clip that already fits is sent as it is.
//! 2. Upload to Catbox (permanent), Litterbox (temporary), a Discord webhook
//!    (posts the file into a channel) or Streamable, with progress.
//! 3. The link is copied to the clipboard and remembered for that clip. A
//!    link from Catbox/Litterbox/Streamable can also be posted to the webhook,
//!    which plays inline in Discord with no size limit and no Nitro.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Catbox,
    Litterbox,
    Discord,
    Streamable,
}

impl Host {
    pub fn parse(s: &str) -> Option<Host> {
        match s {
            "catbox" => Some(Host::Catbox),
            "litterbox" => Some(Host::Litterbox),
            "discord" => Some(Host::Discord),
            "streamable" => Some(Host::Streamable),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Host::Catbox => "Catbox",
            Host::Litterbox => "Litterbox",
            Host::Discord => "Discord",
            Host::Streamable => "Streamable",
        }
    }
    /// Largest file the host takes (bytes).
    fn limit(self) -> u64 {
        match self {
            Host::Catbox => CATBOX_LIMIT,
            Host::Litterbox => 1_000 * 1_000_000,
            // Discord without Nitro or a boosted server: 10 MB.
            Host::Discord => 10 * 1024 * 1024,
            Host::Streamable => 250 * 1_000_000,
        }
    }
}

/// Where each host's API lives (tests point these at a local fake server).
#[derive(Clone)]
pub struct Endpoints {
    pub catbox: String,
    pub litterbox: String,
    pub streamable: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            catbox: "https://catbox.moe/user/api.php".into(),
            litterbox: "https://litterbox.catbox.moe/resources/internals/api.php".into(),
            streamable: "https://api.streamable.com/upload".into(),
        }
    }
}

/// What an upload needs besides the file.
#[derive(Clone, Default)]
pub struct Config {
    /// "1h" | "12h" | "24h" | "72h"
    pub litterbox_time: String,
    pub discord_webhook: Option<String>,
    pub streamable_email: String,
    pub streamable_password: Option<String>,
    /// Also post the link (from Catbox/Litterbox/Streamable) to the Discord webhook
    pub post_link_to_discord: bool,
    /// Shown above the clip in Discord, e.g. "VALORANT clip"
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub host: Host,
    pub url: String,
    /// ms since 1970
    pub at: i64,
    pub bytes: u64,
    /// Litterbox links stop working after this (ms since 1970)
    pub expires: Option<i64>,
}

/// A webhook address has to be one of Discord's, so a clip can't be sent anywhere else by mistake.
pub fn valid_webhook(url: &str) -> bool {
    ["https://discord.com/api/webhooks/", "https://discordapp.com/api/webhooks/", "https://canary.discord.com/api/webhooks/", "https://ptb.discord.com/api/webhooks/"]
        .iter()
        .any(|p| url.starts_with(p) && url.len() > p.len() + 10)
}

/// Make the file a host can take and Discord can play: H.264 MP4, under the size limit.
/// Returns the file to send (the clip itself when it already fits).
pub fn prepare(path: &str, host: Host, work: &Path) -> Result<PathBuf, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    // Catbox keeps clips for good, so a clip too big for it isn't squeezed (a slow,
    // lossy re-encode): it goes to Litterbox, or is trimmed first.
    if host == Host::Catbox && size > host.limit() {
        return Err(too_big_for_catbox(size));
    }
    let p = crate::library::probe(path)?;
    let is_mp4 = Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    let playable = p.codec == "h264" && is_mp4 && p.audio_tracks <= 1;
    if playable && size <= host.limit() {
        return Ok(PathBuf::from(path));
    }
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    let out = work.join(format!("{stem}.mp4"));
    let _ = fs::remove_file(&out);
    if host == Host::Discord {
        // 9 MB target leaves room under Discord's 10 MB.
        crate::library::compress_into(path, 9, out.clone(), &Default::default())?;
    } else {
        crate::library::export_h264_into(path, out.clone(), &Default::default())?;
        let made = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        if host == Host::Catbox && made > host.limit() {
            let _ = fs::remove_file(&out);
            return Err(too_big_for_catbox(made));
        }
        if made > host.limit() {
            let mb = (host.limit() / 1_000_000 * 9 / 10) as u32;
            let squeezed = work.join(format!("{stem} ({mb} MB).mp4"));
            crate::library::compress_into(path, mb, squeezed.clone(), &Default::default())?;
            let _ = fs::remove_file(&out);
            return Ok(squeezed);
        }
    }
    let made = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if made > host.limit() {
        return Err(format!("Couldn't get the clip under {} MB for {}.", host.limit() / 1_000_000, host.label()));
    }
    Ok(out)
}

/// Catbox takes up to 200 MB.
pub const CATBOX_LIMIT: u64 = 200 * 1_000_000;

fn too_big_for_catbox(bytes: u64) -> String {
    format!(
        "This clip is {} MB, and Catbox takes up to {} MB. Upload it to Litterbox (up to 1 GB) instead, or trim it first.",
        bytes.div_ceil(1_000_000),
        CATBOX_LIMIT / 1_000_000
    )
}

/// Counts bytes as reqwest reads the file, for a progress bar.
struct Progress<R> {
    inner: R,
    sent: u64,
    total: u64,
    last: Instant,
    report: Box<dyn FnMut(u64, u64) + Send>,
}

impl<R: Read> Read for Progress<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.sent += n as u64;
        if self.last.elapsed() > Duration::from_millis(150) || n == 0 {
            self.last = Instant::now();
            (self.report)(self.sent, self.total);
        }
        Ok(n)
    }
}

fn file_part(file: &Path, report: Box<dyn FnMut(u64, u64) + Send>) -> Result<reqwest::blocking::multipart::Part, String> {
    let total = fs::metadata(file).map_err(|e| e.to_string())?.len();
    let f = fs::File::open(file).map_err(|e| e.to_string())?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "clip.mp4".into());
    reqwest::blocking::multipart::Part::reader_with_length(Progress { inner: f, sent: 0, total, last: Instant::now(), report }, total)
        .file_name(name)
        .mime_str("video/mp4")
        .map_err(|e| e.to_string())
}

fn client() -> Result<reqwest::blocking::Client, String> {
    // The same TLS setup as the updater (rustls with ring).
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // Big files on slow connections take a while; give up only if nothing moves for long.
        .timeout(None)
        .user_agent(concat!("Hindsight/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn friendly(host: Host, status: reqwest::StatusCode, body: &str) -> String {
    let what = match status.as_u16() {
        401 | 403 => "it refused the login or webhook address",
        404 => "the address wasn't found (deleted webhook?)",
        413 => "the file is too big for it",
        429 => "it's rate-limiting uploads, try again in a minute",
        500..=599 => "it's having trouble right now",
        _ => "it said no",
    };
    let detail: String = body.chars().take(140).collect();
    format!("{} upload failed: {what} ({status}{}{detail})", host.label(), if detail.is_empty() { "" } else { ": " })
}

/// Upload `file` and return the link to share.
pub fn send(host: Host, file: &Path, cfg: &Config, ep: &Endpoints, report: Box<dyn FnMut(u64, u64) + Send>) -> Result<String, String> {
    use reqwest::blocking::multipart::Form;
    let c = client()?;
    let part = file_part(file, report)?;
    let res = match host {
        Host::Catbox => c.post(&ep.catbox).multipart(Form::new().text("reqtype", "fileupload").part("fileToUpload", part)).send(),
        Host::Litterbox => {
            let t = if ["1h", "12h", "24h", "72h"].contains(&cfg.litterbox_time.as_str()) { cfg.litterbox_time.clone() } else { "72h".into() };
            c.post(&ep.litterbox).multipart(Form::new().text("reqtype", "fileupload").text("time", t).part("fileToUpload", part)).send()
        }
        Host::Discord => {
            let hook = cfg.discord_webhook.as_deref().filter(|h| valid_webhook(h) || h.starts_with("http://127.0.0.1")).ok_or("Add a Discord webhook in Settings → Sharing first.")?;
            let payload = serde_json::json!({ "content": cfg.message, "allowed_mentions": { "parse": [] } }).to_string();
            let url = format!("{hook}{}wait=true", if hook.contains('?') { "&" } else { "?" });
            c.post(url).multipart(Form::new().text("payload_json", payload).part("files[0]", part)).send()
        }
        Host::Streamable => {
            let pw = cfg.streamable_password.as_deref().filter(|_| !cfg.streamable_email.is_empty()).ok_or("Add your Streamable email and password in Settings → Sharing first.")?;
            c.post(&ep.streamable).basic_auth(&cfg.streamable_email, Some(pw)).multipart(Form::new().part("file", part)).send()
        }
    }
    .map_err(|e| format!("{} upload failed: couldn't connect ({e})", host.label()))?;
    let status = res.status();
    let body = res.text().unwrap_or_default();
    if !status.is_success() {
        return Err(friendly(host, status, &body));
    }
    let url = match host {
        Host::Catbox | Host::Litterbox => body.trim().to_string(),
        Host::Discord => serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|j| j["attachments"][0]["url"].as_str().map(str::to_string))
            .unwrap_or_default(),
        Host::Streamable => serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|j| j["shortcode"].as_str().map(|s| format!("https://streamable.com/{s}")))
            .unwrap_or_default(),
    };
    if !url.starts_with("http") {
        let detail: String = body.chars().take(140).collect();
        return Err(format!("{} didn't send back a link ({detail})", host.label()));
    }
    // Share the link in Discord too: it plays inline there, whatever the size.
    if host != Host::Discord && cfg.post_link_to_discord {
        if let Some(hook) = cfg.discord_webhook.as_deref().filter(|h| valid_webhook(h) || h.starts_with("http://127.0.0.1")) {
            let content = if cfg.message.is_empty() { url.clone() } else { format!("{}\n{url}", cfg.message) };
            let r = c.post(hook).json(&serde_json::json!({ "content": content, "allowed_mentions": { "parse": [] } })).send();
            if let Ok(r) = r
                && !r.status().is_success()
            {
                crate::log!("[rewind] posting the link to Discord failed: {}", r.status());
            }
        }
    }
    Ok(url)
}

/// Can this PC reach a host? Ad-blocking DNS (Pi-hole, AdGuard, some routers)
/// answers 0.0.0.0 for sites on its blocklist; files.catbox.moe is on many.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    pub host: String,
    /// "ok" | "blocked" (DNS says 0.0.0.0) | "unreachable" (no answer)
    pub state: &'static str,
}

pub fn reach(host: &str) -> Reach {
    use std::net::ToSocketAddrs;
    let state = match (host, 443).to_socket_addrs() {
        Ok(addrs) => {
            let v: Vec<_> = addrs.collect();
            if v.is_empty() {
                "unreachable"
            } else if v.iter().all(|a| a.ip().is_unspecified() || a.ip().is_loopback()) {
                "blocked"
            } else {
                "ok"
            }
        }
        Err(_) => "unreachable",
    };
    Reach { host: host.to_string(), state }
}

/// The names each service needs: where uploads go, and where the links point.
pub fn needed_hosts(h: Host) -> &'static [&'static str] {
    match h {
        Host::Catbox => &["catbox.moe", "files.catbox.moe"],
        Host::Litterbox => &["litterbox.catbox.moe", "litter.catbox.moe"],
        Host::Discord => &["discord.com", "cdn.discordapp.com"],
        Host::Streamable => &["api.streamable.com", "streamable.com"],
    }
}

/// A plain-words warning if a fresh link won't open on this network.
pub fn link_warning(url: &str) -> Option<String> {
    let host = url.split("://").nth(1)?.split('/').next()?;
    match reach(host).state {
        "blocked" => Some(format!(
            "Your network blocks {host} (an ad-blocking DNS like Pi-hole or AdGuard), so the link won't open on this Wi-Fi. Friends elsewhere can watch it. Allow {host} in your blocker, or use Litterbox."
        )),
        "unreachable" => Some(format!("This PC can't look up {host} right now, so the link may not open here.")),
        _ => None,
    }
}

/// What a Discord webhook says about itself (GET on its address needs no login).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WebhookInfo {
    pub name: String,
    pub channel_id: String,
}

pub fn check_webhook(url: &str) -> Result<WebhookInfo, String> {
    if !valid_webhook(url) {
        return Err("That isn't a Discord webhook address. It starts with https://discord.com/api/webhooks/".into());
    }
    let r = client()?.get(url).send().map_err(|e| format!("Couldn't reach Discord ({e})."))?;
    let status = r.status();
    let body = r.text().unwrap_or_default();
    if status.as_u16() == 404 || status.as_u16() == 401 {
        return Err("Discord says that webhook doesn't exist (deleted, or the address was cut off). Copy it again.".into());
    }
    if !status.is_success() {
        return Err(format!("Discord didn't accept that webhook ({status})."));
    }
    let j: serde_json::Value = serde_json::from_str(&body).map_err(|_| "Discord sent back something unexpected.".to_string())?;
    if j["type"].as_i64() != Some(1) || j["token"].as_str().is_none() {
        return Err("That webhook can't post messages (it's a channel-follower webhook). Make a new one.".into());
    }
    Ok(WebhookInfo {
        name: j["name"].as_str().unwrap_or("Webhook").to_string(),
        channel_id: j["channel_id"].as_str().unwrap_or_default().to_string(),
    })
}

/// Discord attachment links stop working after a while: their "ex" value is the
/// expiry time (hex seconds since 1970).
pub fn discord_link_expiry(url: &str) -> Option<i64> {
    let q = url.split('?').nth(1)?;
    let ex = q.split('&').find_map(|kv| kv.strip_prefix("ex="))?;
    i64::from_str_radix(ex, 16).ok().map(|s| s * 1000)
}

/// Put text on the Windows clipboard (works with the window hidden, unlike the web clipboard).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // Another app may hold the clipboard for a moment.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        if !opened {
            return Err("The clipboard is busy.".into());
        }
        let res = (|| -> Result<(), String> {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| e.to_string())?;
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                return Err("Couldn't lock clipboard memory.".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).map_err(|e| e.to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        res
    }
}

/// Links made for each clip, newest last (uploads.json in the app's data folder).
pub fn history(file: &Path) -> std::collections::BTreeMap<String, Vec<Upload>> {
    fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn remember(file: &Path, clip: &str, u: Upload) {
    let mut all = history(file);
    let list = all.entry(clip.to_string()).or_default();
    list.push(u);
    if list.len() > 10 {
        list.remove(0);
    }
    if let Some(d) = file.parent() {
        let _ = fs::create_dir_all(d);
    }
    let tmp = file.with_extension("json.tmp");
    if let Ok(t) = serde_json::to_string_pretty(&all)
        && fs::write(&tmp, t).is_ok()
    {
        let _ = fs::rename(&tmp, file);
    }
}

pub fn litterbox_expiry(t: &str, now_ms: i64) -> i64 {
    let h = match t {
        "1h" => 1,
        "12h" => 12,
        "24h" => 24,
        _ => 72,
    };
    now_ms + h * 3_600_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A one-request fake host: records the request, answers with `status` and `body`.
    fn fake(status: u16, body: &'static str) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in l.incoming().take(2) {
                let Ok(mut s) = stream else { continue };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut body_in = vec![0u8; len];
                let _ = r.read_exact(&mut body_in);
                let _ = tx.send((head, body_in));
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(reply.as_bytes());
            }
        });
        (url, rx)
    }

    fn clip_file(bytes: usize) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("rewind-upload-{}-{n}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("clip.mp4");
        fs::write(&p, vec![42u8; bytes]).unwrap();
        p
    }

    fn eps(url: &str) -> Endpoints {
        Endpoints { catbox: url.into(), litterbox: url.into(), streamable: url.into() }
    }

    #[test]
    fn catbox_sends_the_file_and_returns_the_link() {
        let (url, rx) = fake(200, "https://files.catbox.moe/abc123.mp4\n");
        let file = clip_file(300_000);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(0u64));
        let s2 = seen.clone();
        let link = send(Host::Catbox, &file, &Config::default(), &eps(&url), Box::new(move |sent, _| *s2.lock().unwrap() = sent)).unwrap();
        assert_eq!(link, "https://files.catbox.moe/abc123.mp4");
        let (head, body) = rx.recv().unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(head.starts_with("POST "), "{head}");
        assert!(body.contains("name=\"reqtype\"") && body.contains("fileupload"));
        assert!(body.contains("name=\"fileToUpload\"; filename=\"clip.mp4\""));
        assert!(body.contains("Content-Type: video/mp4"));
        assert_eq!(*seen.lock().unwrap(), 300_000, "progress reached the whole file");
    }

    #[test]
    fn litterbox_passes_how_long_to_keep_it() {
        let (url, rx) = fake(200, "https://litter.catbox.moe/xyz.mp4");
        let cfg = Config { litterbox_time: "24h".into(), ..Default::default() };
        let link = send(Host::Litterbox, &clip_file(1000), &cfg, &eps(&url), Box::new(|_, _| {})).unwrap();
        assert_eq!(link, "https://litter.catbox.moe/xyz.mp4");
        let body = String::from_utf8_lossy(&rx.recv().unwrap().1).to_string();
        assert!(body.contains("name=\"time\"\r\n\r\n24h"), "{body}");
    }

    #[test]
    fn discord_webhook_posts_the_file_and_returns_its_link() {
        let (url, rx) = fake(200, r#"{"id":"1","attachments":[{"url":"https://cdn.discordapp.com/attachments/1/2/clip.mp4"}]}"#);
        let cfg = Config { discord_webhook: Some(format!("{url}/api/webhooks/1/tok")), message: "VALORANT clip".into(), ..Default::default() };
        let link = send(Host::Discord, &clip_file(1000), &cfg, &eps(&url), Box::new(|_, _| {})).unwrap();
        assert_eq!(link, "https://cdn.discordapp.com/attachments/1/2/clip.mp4");
        let (head, body) = rx.recv().unwrap();
        assert!(head.contains("/api/webhooks/1/tok?wait=true"), "{head}");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("name=\"files[0]\"") && body.contains("VALORANT clip") && body.contains("\"parse\":[]"));
    }

    #[test]
    fn streamable_logs_in_and_builds_the_link() {
        let (url, rx) = fake(200, r#"{"shortcode":"q1w2e3","status":1}"#);
        let cfg = Config { streamable_email: "me@example.com".into(), streamable_password: Some("pw".into()), ..Default::default() };
        let link = send(Host::Streamable, &clip_file(1000), &cfg, &eps(&url), Box::new(|_, _| {})).unwrap();
        assert_eq!(link, "https://streamable.com/q1w2e3");
        let head = rx.recv().unwrap().0.to_lowercase();
        assert!(head.contains("authorization: basic "), "{head}");
    }

    #[test]
    fn a_link_can_also_go_to_discord() {
        let (url, rx) = fake(200, "https://files.catbox.moe/abc.mp4");
        let cfg = Config { post_link_to_discord: true, discord_webhook: Some(format!("{url}/hook")), message: "clip".into(), ..Default::default() };
        send(Host::Catbox, &clip_file(500), &cfg, &eps(&url), Box::new(|_, _| {})).unwrap();
        let _upload = rx.recv().unwrap();
        let (head, body) = rx.recv().unwrap();
        assert!(head.starts_with("POST /hook"), "{head}");
        assert!(String::from_utf8_lossy(&body).contains("https://files.catbox.moe/abc.mp4"));
    }

    #[test]
    fn errors_are_explained() {
        let (url, _rx) = fake(413, "too big");
        let e = send(Host::Catbox, &clip_file(500), &Config::default(), &eps(&url), Box::new(|_, _| {})).unwrap_err();
        assert!(e.contains("too big for it"), "{e}");
        let (url, _rx) = fake(200, "<html>oops</html>");
        let e = send(Host::Catbox, &clip_file(500), &Config::default(), &eps(&url), Box::new(|_, _| {})).unwrap_err();
        assert!(e.contains("didn't send back a link"), "{e}");
        let e = send(Host::Discord, &clip_file(500), &Config::default(), &Endpoints::default(), Box::new(|_, _| {})).unwrap_err();
        assert!(e.contains("webhook"), "{e}");
    }

    #[test]
    fn blocked_names_and_discord_expiry_are_recognised() {
        assert_eq!(reach("localhost").state, "blocked", "loopback answer counts as blocked");
        assert_eq!(reach("no-such-host.invalid").state, "unreachable");
        assert!(link_warning("https://localhost/x.mp4").unwrap().contains("blocks localhost"));
        let u = "https://cdn.discordapp.com/attachments/1/2/clip.mp4?ex=6912a3b0&is=69115230&hm=abc";
        assert_eq!(discord_link_expiry(u), Some(0x6912a3b0 * 1000));
        assert_eq!(discord_link_expiry("https://files.catbox.moe/a.mp4"), None);
        assert!(check_webhook("https://example.com/hook").is_err());
    }

    #[test]
    fn only_discord_webhook_addresses_are_accepted() {
        assert!(valid_webhook("https://discord.com/api/webhooks/123456/abcdefghijkl"));
        assert!(!valid_webhook("https://evil.example/api/webhooks/123456/abcdefghijkl"));
        assert!(!valid_webhook("http://discord.com/api/webhooks/123456/abcdefghijkl"));
    }

    #[test]
    fn clips_become_discord_playable_mp4s_that_fit() {
        crate::ffmpeg::init(None);
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let d = std::env::temp_dir().join(format!("rewind-upload-prep-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let src = d.join("clip.mkv");
        let mut a = crate::ffmpeg::args(&[
            "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=60",
            "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono",
            "-t", "20", "-map", "0:v", "-map", "1:a", "-map", "2:a", "-c:v", "av1_nvenc", "-cq", "18", "-c:a", "aac",
        ]);
        a.push(src.clone().into());
        crate::ffmpeg::run("ffmpeg", &a).unwrap();
        let s = src.to_string_lossy().into_owned();
        for host in [Host::Catbox, Host::Discord] {
            let out = prepare(&s, host, &d.join("work")).unwrap();
            let p = crate::library::probe(&out.to_string_lossy()).unwrap();
            let size = fs::metadata(&out).unwrap().len();
            println!("{host:?}: {} MB, {} {}x{}", size / 1_000_000, p.codec, p.width, p.height);
            assert_eq!(p.codec, "h264");
            assert_eq!(p.audio_tracks, 1, "tracks mixed into one so every player hears both");
            assert!(out.extension().unwrap() == "mp4");
            assert!(size <= host.limit());
        }
        // Too big for Catbox: refused before any work, with the way out.
        let big = d.join("big.mp4");
        fs::File::create(&big).unwrap().set_len(CATBOX_LIMIT + 1).unwrap();
        let e = prepare(&big.to_string_lossy(), Host::Catbox, &d.join("work")).unwrap_err();
        assert!(e.contains("Catbox takes up to 200 MB") && e.contains("Litterbox"), "{e}");
        // An MP4 that already fits goes up as it is.
        let ready = prepare(&s, Host::Catbox, &d.join("work")).unwrap();
        assert_eq!(prepare(&ready.to_string_lossy(), Host::Catbox, &d.join("work2")).unwrap(), ready);
        let _ = fs::remove_dir_all(d);
    }
}
