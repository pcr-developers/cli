//! Shared helpers used by every capture source — git wrappers, tool-call
//! utilities, file state tracking, and in-memory deduplication. Mirrors
//! `cli/internal/sources/shared/`.

pub mod dedup;
pub mod git;
pub mod state;
pub mod tool_calls;

pub use dedup::*;
pub use git::*;
pub use state::*;
pub use tool_calls::*;
