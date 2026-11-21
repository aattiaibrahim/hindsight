//! Saved secrets (Discord webhook, Streamable password) are encrypted with Windows
//! DPAPI. Elsewhere they'll go in the Keychain / Secret Service (phase 4); until
//! then nothing is stored, rather than storing it in plain text.

use std::path::Path;

pub fn set(_file: &Path, _name: &str, _value: &str) -> Result<(), String> {
    Err("Saving passwords isn't available on this system yet.".into())
}

pub fn get(_file: &Path, _name: &str) -> Option<String> {
    None
}

pub fn is_set(_file: &Path, _name: &str) -> bool {
    false
}
