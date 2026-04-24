//! Centralized exit codes. Every command returns one of these.
//!
//! Codes are deliberately stable and non-overlapping across categories so an
//! agent wrapper (Cursor, Claude Code, Windsurf, Codex) can branch on them.

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Command ran to completion successfully.
    Success = 0,
    /// Generic runtime error — something went wrong but nothing more specific.
    GenericError = 1,
    /// Usage / argument error (invalid flag combination, missing arg, etc).
    Usage = 2,
    /// Command requires an authenticated user; none is logged in.
    AuthRequired = 10,
    /// Network / Supabase / GitHub call failed.
    Network = 20,
    /// A requested resource (draft, bundle, project) doesn't exist.
    NotFound = 30,
    /// An interactive flow is required but stdin / stdout isn't a TTY, and
    /// no flags were supplied to make it non-interactive.
    InteractiveUnavailable = 40,
    /// Feature not yet implemented in the current build.
    NotImplemented = 50,
}

impl From<ExitCode> for i32 {
    fn from(c: ExitCode) -> i32 {
        c as i32
    }
}

impl ExitCode {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}
