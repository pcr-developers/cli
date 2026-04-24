//! Persisted CLI credentials. Direct port of `cli/internal/auth/auth.go`.
//!
//! The on-disk JSON format is identical to the Go version so users who
//! upgrade from a Go build keep their existing login.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    pub token: String,
    #[serde(rename = "userId")]
    pub user_id: String,
}

fn auth_file_path() -> PathBuf {
    config::pcr_dir().join("auth.json")
}

/// Load the saved auth credentials. Returns `None` if the file doesn't
/// exist or can't be parsed — mirrors the Go `Load() *Auth` behavior which
/// returns `nil` on any error.
pub fn load() -> Option<Auth> {
    let data = fs::read(auth_file_path()).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Persist auth credentials to disk with 0600 permissions.
pub fn save(auth: &Auth) -> anyhow::Result<()> {
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(auth)?;
    fs::write(&path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// Clear saved credentials. Silently succeeds if no file exists.
pub fn clear() {
    let _ = fs::remove_file(auth_file_path());
}
