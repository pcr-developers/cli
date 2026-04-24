//! pcr-core: all business logic for the PCR.dev CLI.
//!
//! This crate is consumed by two thin entry-point crates:
//!
//! - [`pcr-cli`] — plain `cargo build` binary used by Homebrew and GitHub Releases.
//! - [`pcr-napi`] — a cdylib loaded by `node.exe` via napi-rs, used by the npm
//!   distribution so that Windows AppLocker never evaluates a PCR-shipped `.exe`.
//!
//! Both entry points call [`entry::run`], which returns a process exit code.

#![allow(
    clippy::needless_return,
    clippy::too_many_arguments,
    clippy::collapsible_match,
    clippy::collapsible_if,
    clippy::implicit_saturating_sub,
    clippy::manual_pattern_char_comparison,
    clippy::manual_repeat_n,
    clippy::doc_lazy_continuation,
    clippy::items_after_test_module,
    clippy::uninlined_format_args,
    clippy::manual_map,
    clippy::needless_range_loop,
    clippy::single_match,
    clippy::new_without_default,
    clippy::too_many_lines
)]

pub mod agent;
pub mod auth;
pub mod config;
pub mod display;
pub mod entry;
pub mod exit;
pub mod mcp;
pub mod projects;
pub mod sources;
pub mod store;
pub mod supabase;
pub mod tui;
pub mod util;
pub mod versions;

pub mod commands;

/// Version string exposed to `--version` and to Supabase telemetry.
/// Release CI overrides this via the `PCR_VERSION` env var passed through
/// `-C env.PCR_VERSION=...`, otherwise we fall back to the workspace version.
pub const VERSION: &str = match option_env!("PCR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Build time, optionally baked in at CI time.
pub const BUILD_TIME: &str = match option_env!("PCR_BUILD_TIME") {
    Some(v) => v,
    None => "",
};

/// Runtime tag that identifies this as the Rust build. Printed next to the
/// version so support can tell which variant a user is running.
pub const RUNTIME: &str = "rust";
