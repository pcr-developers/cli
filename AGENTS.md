# AI-assisted contribution notes

This file is read by Cursor, Claude Code, and similar tools when making
changes inside the repo root. It codifies conventions that are not obvious
from the code alone.

## Shape

- `crates/pcr-core/` is where every command, every watcher, every store
  query, every display routine lives. Nothing else contains logic.
- `crates/pcr-cli/` is a 4-line `main` that calls `pcr_core::entry::run`.
  Do not add logic here.
- `crates/pcr-napi/` is a `#[napi] pub fn run(argv)` shim. Do not add
  logic here either.

## Parity with the Go CLI

The Go implementation under `../cli/` is the behavioral source of truth
during the transition. If you change a command's output, exit code, or
persisted format, update the corresponding Go file first or — if the Go
side is being removed — explicitly note the divergence in the commit
message.

Specifically byte-compatible between the two builds:

- `$HOME/.pcr-dev/auth.json` — JSON shape
- `$HOME/.pcr-dev/projects.json` — JSON shape
- `$HOME/.pcr-dev/drafts.db` — SQLite schema including v1..v6 migrations
- Supabase row `content_hash` — SHA-256 over `session_id\x00prompt\x00response`
- Supabase `bundle_id`, prompt `id` — UUID-shaped from the same hash
- Exit codes — match the enum in `crates/pcr-core/src/exit.rs`

Unit tests in `crates/pcr-core/src/{projects,supabase}.rs` + integration
tests in `crates/pcr-cli/tests/golden.rs` pin this down. Don't regress.

## Output discipline

- **stderr** carries human-readable status, colors, TUI frames.
- **stdout** carries machine-parseable JSON only when `--json` is set.
- Any command that defaults to a TUI must fall back to line output when
  `agent::is_tui_eligible(mode)` returns false. That helper already checks
  TTY, `CI`, `NO_COLOR`, and the explicit flags — don't re-invent it.
- Respect `NO_COLOR` and `FORCE_COLOR` via the `agent::colors_enabled`
  helper, not ad-hoc.

## Port state

Every file under `cli/cmd/` and `cli/internal/` has a corresponding Rust
module here and is a 1:1 port. The capture sources (Claude Code, Cursor,
VS Code) run real file watchers against the user's local editor state
and save drafts identical to the Go build. `compute_incremental_diffs`
in `commands/push.rs` mirrors the Go per-session timeline algorithm.

The only piece intentionally left as a stub is `pcr-core::mcp` — the MCP
server isn't used in production yet. When that changes, implement it
against the `rmcp` crate and remove the `NotImplemented` stub in
`mcp/mod.rs`.

## Adding a dependency

- Add the crate with a pinned version to `Cargo.toml` under
  `[workspace.dependencies]`, then reference it from the member crate's
  `Cargo.toml` as `name = { workspace = true }`.
- Avoid adding async runtimes (tokio, async-std) — the CLI is
  deliberately sync. Only exception is if MCP is being re-introduced.
- Avoid `libc` for portable ops; use `std::process`, `std::fs`, etc.

## Running locally

```bash
cargo build --release -p pcr-cli
./target/release/pcr --help

# Tests
cargo test --workspace

# Build the .node addon for npm path
cd crates/pcr-napi
npx @napi-rs/cli build --platform --release
```
