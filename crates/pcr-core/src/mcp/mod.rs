//! MCP (Model Context Protocol) server — stub.
//!
//! The Go CLI ships a real MCP server ([cli/cmd/mcp.go]) that exposes three
//! tools: `pcr_log_prompt`, `pcr_log_session`, `pcr_status`. The Rust
//! rewrite deliberately defers this until PCR actually consumes MCP in
//! production; `pcr mcp` today prints a clear diagnostic and exits
//! [`crate::exit::ExitCode::NotImplemented`].
//!
//! Resurrecting MCP when it's needed is a bounded piece of work — either
//! via the `rmcp` crate or by vendoring a thin JSON-RPC-over-stdio
//! implementation.

use crate::exit::ExitCode;

pub fn run_stub() -> ExitCode {
    eprintln!("PCR: MCP server is not yet implemented in the Rust build.");
    eprintln!(
        "     When you need it, re-enable it by implementing `pcr-core::mcp` against the `rmcp` crate,"
    );
    eprintln!(
        "     mirroring the three tools in the previous Go build (`pcr_log_prompt`, `pcr_log_session`, `pcr_status`)."
    );
    ExitCode::NotImplemented
}
