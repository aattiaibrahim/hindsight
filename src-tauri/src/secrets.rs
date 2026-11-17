//! Small secrets (a Discord webhook address, a Streamable password), kept
//! encrypted with Windows DPAPI: only this Windows account on this PC can read
//! them back. They live in their own file, never in settings.json, and the
//! interface only ever learns whether one is set.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};

/// Names the app is allowed to store.
pub const NAMES: &[&str] = &["discord_webhook", "streamable_password"];

fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| e.to_string())?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        Ok(v)
    }
}

fn unprotect(blob: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| e.to_string())?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        Ok(v)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn load(file: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Store (or with an empty value, remove) a secret.
pub fn set(file: &Path, name: &str, value: &str) -> Result<(), String> {
    if !NAMES.contains(&name) {
        return Err(format!("unknown secret {name}"));
    }
    let mut all = load(file);
    if value.is_empty() {
        all.remove(name);
    } else {
        all.insert(name.to_string(), hex(&protect(value.as_bytes())?));
    }
    if let Some(d) = file.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = file.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    crate::library::flush_to_disk(&tmp)?;
    fs::rename(&tmp, file).map_err(|e| e.to_string())
}

pub fn get(file: &Path, name: &str) -> Option<String> {
    let blob = unhex(load(file).get(name)?)?;
    String::from_utf8(unprotect(&blob).ok()?).ok()
}

pub fn is_set(file: &Path, name: &str) -> bool {
    load(file).contains_key(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_never_stored_in_plain_text() {
        let d = std::env::temp_dir().join(format!("rewind-secrets-{}", std::process::id()));
        let f = d.join("secrets.json");
        let url = "https://discord.com/api/webhooks/123/abcDEF";
        set(&f, "discord_webhook", url).unwrap();
        assert!(is_set(&f, "discord_webhook"));
        assert_eq!(get(&f, "discord_webhook").as_deref(), Some(url));
        let raw = fs::read_to_string(&f).unwrap();
        assert!(!raw.contains("abcDEF") && !raw.contains("discord.com"), "stored encrypted");
        set(&f, "discord_webhook", "").unwrap();
        assert!(!is_set(&f, "discord_webhook"));
        assert!(set(&f, "anything_else", "x").is_err());
        let _ = fs::remove_dir_all(d);
    }
}
