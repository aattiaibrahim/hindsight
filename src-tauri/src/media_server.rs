//! Plays clips to the app's own player quickly.
//!
//! The WebView loads videos through Tauri's file protocol, which reads the
//! whole file into memory for any request without a byte range (and WebView2
//! sends those for video), then answers in 1 MB pieces. With clips of 400 MB+
//! that made opening one take seconds. This tiny server streams exactly the
//! bytes the player asks for instead.
//!
//! It's locked down:
//! - it listens on 127.0.0.1 only, on a port Windows picks at random;
//! - every address carries a secret made fresh at each start, and the Host
//!   header must be this address (no DNS rebinding), so neither a website nor
//!   another program can read files through it;
//! - it serves video, audio and picture files only, and nothing else.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

static BASE: OnceLock<(u16, String)> = OnceLock::new();

const KINDS: [(&str, &str); 12] = [
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("m4a", "audio/mp4"),
    ("mp3", "audio/mpeg"),
    ("aac", "audio/aac"),
    ("wav", "audio/wav"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
];

fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    KINDS.iter().find(|(e, _)| *e == ext).map(|(_, k)| *k)
}

/// A fresh secret for this run: hashed from the time, the process, and a
/// randomly seeded hasher.
fn secret() -> String {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};
    let mut h = Sha256::new();
    let mut r = std::collections::hash_map::RandomState::new().build_hasher();
    r.write_u64(std::process::id() as u64);
    h.update(r.finish().to_le_bytes());
    h.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos().to_le_bytes());
    h.update(std::collections::hash_map::RandomState::new().build_hasher().finish().to_le_bytes());
    h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

pub fn start() {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
        crate::log!("media server unavailable; clips play through the file protocol");
        return;
    };
    let Ok(port) = listener.local_addr().map(|a| a.port()) else { return };
    let token = secret();
    let _ = BASE.set((port, token.clone()));
    std::thread::Builder::new()
        .name("hindsight-media".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let token = token.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, port, &token);
                });
            }
        })
        .ok();
}

/// "http://127.0.0.1:<port>/m/<secret>?p=", for the app's player; None if it isn't running.
pub fn base() -> Option<String> {
    BASE.get().map(|(port, token)| format!("http://127.0.0.1:{port}/m/{token}?p="))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// "bytes=START-END" (END optional) → (start, end) inclusive, within `len`.
fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // one range at a time is all a player asks for
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?; // the last N bytes
        (len.saturating_sub(n), len.checked_sub(1)?)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { len.checked_sub(1)? } else { b.parse::<u64>().ok()?.min(len.checked_sub(1)?) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}

/// The app's own page (built, or the dev server). Studio plays a clip through Web Audio
/// to turn a track up past 100% while you edit, and that needs the file served with CORS.
fn app_origin(o: &str) -> bool {
    matches!(o, "http://tauri.localhost" | "https://tauri.localhost" | "tauri://localhost")
        || o.strip_prefix("http://localhost:").is_some_and(|p| p.parse::<u16>().is_ok())
}

fn reply(stream: &mut TcpStream, code: &str) -> std::io::Result<()> {
    write!(stream, "HTTP/1.1 {code}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

fn serve(mut stream: TcpStream, port: u16, token: &str) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let (mut host, mut range, mut origin) = (String::new(), None::<String>, None::<String>);
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim().to_string(),
                "range" => range = Some(v.trim().to_string()),
                "origin" => origin = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    if method != "GET" && method != "HEAD" {
        return reply(&mut stream, "405 Method Not Allowed");
    }
    // Only this exact address: a website pointing its own name at 127.0.0.1 doesn't get in.
    if host != format!("127.0.0.1:{port}") {
        return reply(&mut stream, "403 Forbidden");
    }
    let Some(rest) = target.strip_prefix(&format!("/m/{token}?p=")) else {
        return reply(&mut stream, "403 Forbidden");
    };
    // Anything after the path (a cache-buster like &v=…) is ignored.
    let raw = rest.split('&').next().unwrap_or("");
    let path = percent_decode(raw);
    let p = Path::new(&path);
    let Some(kind) = kind_of(p) else {
        return reply(&mut stream, "403 Forbidden");
    };
    let Ok(mut file) = std::fs::File::open(p) else {
        return reply(&mut stream, "404 Not Found");
    };
    let len = file.metadata()?.len();
    let mut common = format!("Content-Type: {kind}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n");
    if let Some(o) = origin.as_deref().filter(|o| app_origin(o)) {
        common += &format!("Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n");
    }
    let (status, start, end) = match range.as_deref() {
        None => ("200 OK", 0, len.saturating_sub(1)),
        Some(r) => match parse_range(r, len) {
            Some((s, e)) => ("206 Partial Content", s, e),
            None => {
                return write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        },
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    let mut head = format!("HTTP/1.1 {status}\r\n{common}Content-Length: {n}\r\n");
    if status.starts_with("206") {
        head += &format!("Content-Range: bytes {start}-{end}/{len}\r\n");
    }
    head += "\r\n";
    stream.write_all(head.as_bytes())?;
    if method == "HEAD" || n == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    // Streamed straight from the file (the player usually stops reading early), in
    // 1 MB reads: over a network share (the NAS) each read is a round trip, and
    // io::copy's 8 KB ones got about 13 MB/s where 1 MB ones get 65-75 MB/s.
    stream_out(&mut file.take(n), &mut stream)
}

fn stream_out(from: &mut impl Read, to: &mut impl Write) -> std::io::Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let got = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..got])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_origins() {
        assert!(app_origin("http://tauri.localhost"));
        assert!(app_origin("http://localhost:1420"));
        assert!(!app_origin("http://localhost:1420.evil.com"));
        assert!(!app_origin("https://example.com"));
    }

    #[test]
    fn ranges_and_paths() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-500", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(percent_decode("C%3A%5CUsers%5Cx%20y%5Cclip.mkv"), r"C:\Users\x y\clip.mkv");
        assert!(kind_of(Path::new("a.MKV")).is_some());
        assert!(kind_of(Path::new("settings.json")).is_none());
    }

    #[test]
    fn serves_ranges_and_refuses_strangers() {
        start();
        let (port, token) = BASE.get().unwrap().clone();
        let dir = std::env::temp_dir().join(format!("hindsight-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, (0..=255u8).collect::<Vec<_>>()).unwrap();
        std::fs::write(dir.join("secret.txt"), b"no").unwrap();
        let get = |path: &str, host: &str, extra: &str| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n").unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            out
        };
        let enc = |p: &Path| p.to_string_lossy().bytes().map(|b| format!("%{b:02X}")).collect::<String>();
        let me = format!("127.0.0.1:{port}");
        let ok = get(&format!("/m/{token}?p={}", enc(&clip)), &me, "Range: bytes=10-13\r\n");
        let text = String::from_utf8_lossy(&ok);
        assert!(text.starts_with("HTTP/1.1 206"), "{text}");
        assert!(ok.ends_with(&[10, 11, 12, 13]));
        // Wrong secret, wrong Host, or a file that isn't a clip: refused.
        assert!(String::from_utf8_lossy(&get(&format!("/m/nope?p={}", enc(&clip)), &me, "")).starts_with("HTTP/1.1 403"));
        assert!(String::from_utf8_lossy(&get(&format!("/m/{token}?p={}", enc(&clip)), "evil.example:80", "")).starts_with("HTTP/1.1 403"));
        assert!(String::from_utf8_lossy(&get(&format!("/m/{token}?p={}", enc(&dir.join("secret.txt"))), &me, "")).starts_with("HTTP/1.1 403"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live {
    /// Serves for 25 s and prints its address, to time a real clip in a browser:
    /// cargo test --lib live_media_server -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_media_server() {
        super::start();
        println!("BASE {}", super::base().unwrap());
        std::thread::sleep(std::time::Duration::from_secs(25));
    }
}

#[cfg(test)]
mod speed {
    /// MEDIA_SPEED_FILE=<clip on the NAS>: 40 MB through the old 8 KB copy and the new 1 MB reads.
    #[test]
    #[ignore]
    fn nas_read_speed() {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(p) = std::env::var("MEDIA_SPEED_FILE") else { return };
        let n = 40u64 << 20;
        for (name, offset) in [("8 KB copy", 0u64), ("1 MB reads", 60 << 20)] {
            let mut f = std::fs::File::open(&p).unwrap();
            f.seek(SeekFrom::Start(offset)).unwrap();
            let t = std::time::Instant::now();
            let mut sink = std::io::sink();
            if name.starts_with("8") {
                std::io::copy(&mut f.take(n), &mut sink).unwrap();
            } else {
                super::stream_out(&mut f.take(n), &mut sink).unwrap();
            }
            eprintln!("{name}: {:.0} MB/s", 40.0 / t.elapsed().as_secs_f64());
        }
    }
}
