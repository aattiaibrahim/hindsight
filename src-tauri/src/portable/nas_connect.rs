//! Signing in to a NAS share uses Windows networking (WNet, Credential Manager).
//! Elsewhere the NAS is a folder the system has already mounted (gvfs / cifs on
//! Linux, /Volumes on macOS), which works as is; signing in from Hindsight is phase 4.

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NasShare {
    pub name: String,
    pub comment: String,
    pub path: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SavedLogin {
    pub host: String,
    pub user: String,
}

const MOUNT_IT: &str = "On this system, mount the NAS share first (in your file manager), then pick that folder.";

pub fn login_rejected() -> bool {
    false
}

pub fn saved_login(_host: &str) -> Option<SavedLogin> {
    None
}

pub fn forget(_host: &str) {}

pub fn connect(_host: &str, _user: &str, _password: &str, _save: bool) -> Result<Vec<NasShare>, String> {
    Err(MOUNT_IT.into())
}

pub fn open_share(_share_path: &str, _user: &str, _password: &str) -> Result<String, String> {
    Err(MOUNT_IT.into())
}

pub fn reconnect(_dir: &str) -> bool {
    false
}
