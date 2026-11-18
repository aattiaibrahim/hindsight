//! Signing in to the NAS, the way File Explorer's "Map network drive → Connect
//! using different credentials" does it, so nobody has to type a `\\` path or
//! open the share in Explorer first.
//!
//! - The NAS address, user name and password go to Windows' own networking
//!   (WNetAddConnection2), the same SMB connection Explorer makes. Copying
//!   speed is exactly what it was.
//! - With "Remember", the password is kept in Windows Credential Manager as a
//!   Windows credential for that NAS, like Explorer's "Remember my
//!   credentials". Windows then signs in to that NAS by itself after a restart,
//!   for Hindsight and for Explorer alike. Hindsight never stores it anywhere.
//! - The share list comes from asking the NAS directly (NetShareEnum), so
//!   Windows' "Network discovery" doesn't need to be on.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{FILETIME, WIN32_ERROR};
use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows::Win32::NetworkManagement::WNet::{
    WNetAddConnection2W, WNetCancelConnection2W, NETRESOURCEW, NET_CONNECT_FLAGS, RESOURCETYPE_DISK,
};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_DOMAIN_PASSWORD,
};
use windows::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE, STYPE_MASK};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NasShare {
    pub name: String,
    /// The NAS's own description of the share ("Comment" on TrueNAS), may be empty.
    pub comment: String,
    /// `\\host\name`
    pub path: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SavedLogin {
    pub host: String,
    pub user: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(p: PWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { p.to_string().unwrap_or_default() }
    }
}

/// `\\nas\clips\x`, `smb://nas/`, `http://192.168.0.5:8420`, `nas` → `nas`.
pub fn clean_host(input: &str) -> String {
    let mut h = input.trim();
    for p in ["smb://", "http://", "https://", "file://"] {
        if h.len() >= p.len() && h[..p.len()].eq_ignore_ascii_case(p) {
            h = &h[p.len()..];
        }
    }
    let h = h.trim_start_matches(['\\', '/']);
    let h = h.split(['\\', '/']).next().unwrap_or("");
    // A port belongs to the web app, not to file sharing. (IPv6 isn't supported here.)
    h.split(':').next().unwrap_or("").to_string()
}

/// The host of a `\\host\share\...` path.
pub fn host_of(dir: &str) -> Option<String> {
    let rest = dir.strip_prefix("\\\\")?;
    let h = rest.split('\\').next()?;
    (!h.is_empty()).then(|| h.to_string())
}

/// `\\host\share` of a `\\host\share\...` path.
fn share_root(dir: &str) -> Option<String> {
    let rest = dir.strip_prefix("\\\\")?;
    let mut it = rest.split('\\').filter(|p| !p.is_empty());
    let (h, s) = (it.next()?, it.next()?);
    Some(format!("\\\\{h}\\{s}"))
}

/// A Windows error as something a person can act on.
fn explain(code: u32, host: &str) -> String {
    match code {
        5 => "The NAS said no: this user can't open that share. On the NAS, give the user access to it.".into(),
        53 | 1203 => format!("Couldn't find \"{host}\" on the network. Is the NAS on? Try its IP address (like 192.168.0.10)."),
        64 | 1231 | 1232 | 1222 => format!("\"{host}\" didn't answer. Is the NAS on and this PC on the same network?"),
        67 => "That share doesn't exist on the NAS (any more).".into(),
        86 | 1326 | 2202 => "Wrong user name or password. On TrueNAS, the user needs \"SMB User\" ticked.".into(),
        1219 => format!(
            "Windows is already signed in to \"{host}\" as a different user, and only allows one at a time. \
             Sign out of it in File Explorer (or restart the PC), or use the user Windows is signed in with."
        ),
        1244 => "The NAS wants a user name and password.".into(),
        1311 | 1355 => "The NAS asked for a domain sign-in this PC can't reach. Use a user made on the NAS itself.".into(),
        1907 | 1330 | 1909 => "That account's password has expired or the account is locked on the NAS.".into(),
        _ => format!("Couldn't connect to the NAS (Windows error {code})."),
    }
}

/// Connect `remote` (`\\host\share` or `\\host\IPC$`). Empty user and password:
/// whatever Windows already has for that NAS (the saved credential).
fn add_connection(remote: &str, user: &str, password: &str) -> u32 {
    let remote_w = wide(remote);
    let nr = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        lpRemoteName: PWSTR(remote_w.as_ptr() as *mut u16),
        ..Default::default()
    };
    let (user_w, pass_w) = (wide(user), wide(password));
    let user_p = if user.is_empty() { PCWSTR::null() } else { PCWSTR(user_w.as_ptr()) };
    let pass_p = if user.is_empty() && password.is_empty() { PCWSTR::null() } else { PCWSTR(pass_w.as_ptr()) };
    // IPC$ is a "printer/other" resource to Windows, not a disk.
    let nr = if remote.to_ascii_uppercase().ends_with("\\IPC$") {
        NETRESOURCEW { dwType: windows::Win32::NetworkManagement::WNet::RESOURCETYPE_ANY, ..nr }
    } else {
        nr
    };
    // No drive letter, not remembered as a mapped drive: the credential below does that.
    let r: WIN32_ERROR = unsafe { WNetAddConnection2W(&nr, pass_p, user_p, NET_CONNECT_FLAGS(0)) };
    r.0
}

fn cancel_connection(remote: &str) {
    let w = wide(remote);
    unsafe {
        let _ = WNetCancelConnection2W(PCWSTR(w.as_ptr()), NET_CONNECT_FLAGS(0), true);
    }
}

/// Sign in to `host` with these details; on "already signed in as someone else",
/// drop Hindsight's own earlier connections to it and try once more.
fn sign_in(host: &str, remote: &str, user: &str, password: &str) -> Result<(), String> {
    let mut code = add_connection(remote, user, password);
    if code == 1219 {
        cancel_connection(&format!("\\\\{host}\\IPC$"));
        cancel_connection(remote);
        code = add_connection(remote, user, password);
    }
    match code {
        0 | 85 /* already connected */ => Ok(()),
        c => Err(explain(c, host)),
    }
}

fn shares_of(host: &str) -> Result<Vec<NasShare>, String> {
    let server = wide(&format!("\\\\{host}"));
    let mut buf: *mut u8 = std::ptr::null_mut();
    let (mut read, mut total) = (0u32, 0u32);
    let code = unsafe { NetShareEnum(PCWSTR(server.as_ptr()), 1, &mut buf, u32::MAX, &mut read, &mut total, None) };
    if code != 0 {
        if !buf.is_null() {
            unsafe {
                NetApiBufferFree(Some(buf as *const _));
            }
        }
        return Err(explain(code, host));
    }
    let mut out = Vec::new();
    if !buf.is_null() {
        let items = unsafe { std::slice::from_raw_parts(buf as *const SHARE_INFO_1, read as usize) };
        for it in items {
            let name = from_wide(it.shi1_netname);
            // Folders only; skip hidden admin shares (IPC$, C$…).
            if (it.shi1_type.0 & STYPE_MASK.0) != STYPE_DISKTREE.0 || name.ends_with('$') || name.is_empty() {
                continue;
            }
            out.push(NasShare { path: format!("\\\\{host}\\{name}"), comment: from_wide(it.shi1_remark), name });
        }
        unsafe {
            NetApiBufferFree(Some(buf as *const _));
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    Ok(out)
}

/// Keep the password in Credential Manager, as a Windows credential for `host`
/// (what Explorer's "Remember my credentials" makes).
fn remember(host: &str, user: &str, password: &str) -> Result<(), String> {
    let (target, user_w) = (wide(host), wide(user));
    let blob: Vec<u8> = password.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_DOMAIN_PASSWORD,
        TargetName: PWSTR(target.as_ptr() as *mut u16),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user_w.as_ptr() as *mut u16),
        LastWritten: FILETIME::default(),
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("Couldn't save the password in Windows Credential Manager: {e}"))
}

/// The user name Windows has saved for `host`, if any (never the password).
pub fn saved_login(host: &str) -> Option<SavedLogin> {
    let host = clean_host(host);
    if host.is_empty() {
        return None;
    }
    let target = wide(&host);
    let mut p: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_DOMAIN_PASSWORD, None, &mut p).ok()?;
        let user = from_wide((*p).UserName);
        CredFree(p as *const _);
        Some(SavedLogin { host, user })
    }
}

/// Forget the saved password for `host` and drop Hindsight's connections to it.
pub fn forget(host: &str) {
    let host = clean_host(host);
    let target = wide(&host);
    unsafe {
        let _ = CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_DOMAIN_PASSWORD, None);
    }
    cancel_connection(&format!("\\\\{host}\\IPC$"));
}

/// Sign in to the NAS and list its shared folders. With `save`, the password
/// goes to Credential Manager only once it's been accepted. An empty password
/// uses the one already saved for this NAS.
pub fn connect(host: &str, user: &str, password: &str, save: bool) -> Result<Vec<NasShare>, String> {
    let host = clean_host(host);
    if host.is_empty() {
        return Err("Type the NAS's name or IP address.".into());
    }
    let user = user.trim();
    let use_saved = password.is_empty();
    if !use_saved && user.is_empty() {
        return Err("Type the user name you made on the NAS.".into());
    }
    let (u, p) = if use_saved { ("", "") } else { (user, password) };
    sign_in(&host, &format!("\\\\{host}\\IPC$"), u, p)?;
    let shares = shares_of(&host)?;
    if save && !use_saved {
        remember(&host, user, password)?;
    }
    Ok(shares)
}

/// Open a share (or a folder in it) with the details just used to connect, and
/// return the folder's path for the usual NAS folder check.
pub fn open_share(share_path: &str, user: &str, password: &str) -> Result<String, String> {
    let root = share_root(share_path).ok_or("That isn't a NAS folder.")?;
    let host = host_of(&root).unwrap_or_default();
    let (u, p) = if password.is_empty() { ("", "") } else { (user.trim(), password) };
    sign_in(&host, &root, u, p)?;
    let dir = share_path.trim_end_matches('\\').to_string();
    if !std::path::Path::new(&dir).is_dir() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make the folder {dir}: {e}"))?;
    }
    Ok(dir)
}

/// The sync worker calls this when the NAS folder can't be opened: sign in
/// again with the saved credential (after a restart, the NAS rebooting, Wi-Fi
/// coming back). At most once a minute; does nothing without a saved login.
pub fn reconnect(dir: &str) -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return false;
        }
        *last = Some(Instant::now());
    }
    let (Some(root), Some(host)) = (share_root(dir), host_of(dir)) else { return false };
    if saved_login(&host).is_none() {
        return false;
    }
    add_connection(&root, "", "") == 0 && std::path::Path::new(dir).is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_cleaned_from_whatever_people_type() {
        assert_eq!(clean_host("192.168.0.113"), "192.168.0.113");
        assert_eq!(clean_host("  \\\\Voyager-1\\clips\\Valorant "), "Voyager-1");
        assert_eq!(clean_host("smb://nas/clips"), "nas");
        assert_eq!(clean_host("http://192.168.0.113:8420/"), "192.168.0.113");
        assert_eq!(clean_host("//nas"), "nas");
        assert_eq!(clean_host(""), "");
    }

    #[test]
    fn share_roots_and_hosts() {
        assert_eq!(share_root("\\\\nas\\clips\\Valorant").as_deref(), Some("\\\\nas\\clips"));
        assert_eq!(share_root("\\\\nas\\clips").as_deref(), Some("\\\\nas\\clips"));
        assert_eq!(share_root("\\\\nas"), None);
        assert_eq!(share_root("D:\\Clips"), None);
        assert_eq!(host_of("\\\\192.168.0.113\\clips").as_deref(), Some("192.168.0.113"));
        assert_eq!(host_of("D:\\Clips"), None);
    }

    #[test]
    fn errors_say_what_to_do() {
        assert!(explain(1326, "nas").contains("Wrong user name or password"));
        assert!(explain(1219, "nas").contains("only allows one"));
        assert!(explain(53, "nas").contains("Is the NAS on"));
        assert!(explain(4242, "nas").contains("4242"));
    }

    #[test]
    #[ignore = "touches the network; run by hand: cargo test -- --ignored"]
    fn an_unknown_nas_is_an_error_not_a_hang_or_crash() {
        // .invalid never resolves (RFC 2606).
        let r = connect("hindsight-test.invalid", "nobody", "x", false);
        assert!(r.is_err());
    }
}
