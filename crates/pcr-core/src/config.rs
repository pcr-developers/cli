//! Compile-time constants. Mirrors `cli/internal/config/constants.go`.

/// Supabase project URL used for all RPC calls.
pub const SUPABASE_URL: &str = "https://icbsvwffcykzimjjonad.supabase.co";

/// Supabase anon/publishable key.
pub const SUPABASE_KEY: &str = "sb_publishable_1a5u2j7KA23LlX958xW2hw_YE59GfOd";

/// Canonical web app URL used for deep links ("open settings", "review").
pub const APP_URL: &str = "https://pcr.dev";

/// Name of the per-user data directory under `$HOME`.
pub const PCR_DIR: &str = ".pcr-dev";

/// Returns the absolute path of `$HOME/.pcr-dev`, creating nothing on disk.
pub fn pcr_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(PCR_DIR)
}
