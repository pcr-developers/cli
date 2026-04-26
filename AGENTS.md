# AI-assisted contribution notes

This file is read by Cursor, Claude Code, and similar tools when making
changes inside the repo root. It codifies conventions that are not
obvious from the code alone.

## Shape

```
crates/
├── pcr-core/  ← every command, watcher, store query, display routine,
│                 TUI screen — all logic lives here.
├── pcr-cli/   ← 4-line `main` that calls `pcr_core::entry::run`.
│                 Do not add logic here. It exists to ship a single
│                 standalone binary for Homebrew + GitHub Releases.
└── pcr-napi/  ← `#[napi] pub fn run(argv)` shim. Do not add logic
                  here either. It exists so `npm install -g pcr-dev`
                  loads a Rust `.node` addon into `node.exe` (works
                  under Windows AppLocker / WDAC; see NPM-INTERNALS.md).
```

The repo is pure Rust as of v0.2.x. The previous Go implementation was
removed in the rust-port merge commit; if you ever need it, check out
the `v0.1.17` tag.

## On-disk format stability

State written to disk under `$HOME/.pcr-dev/` is shared across versions
and across the (now-archived) Go and Rust builds. **Don't break any of
these without a migration**:

| Path / field | Shape |
|---|---|
| `$HOME/.pcr-dev/auth.json` | JSON object — keep camelCase keys |
| `$HOME/.pcr-dev/projects.json` | JSON array of `{id, name, path, ...}` |
| `$HOME/.pcr-dev/drafts.db` | SQLite, schema v1..vN with sequential migrations |
| Supabase `prompts.content_hash` | SHA-256 over `session_id\x00prompt\x00response` |
| Supabase `bundle_id`, prompt `id` | UUID-shaped, derived from the same hash |
| Exit codes | match the enum in `crates/pcr-core/src/exit.rs` |

The unit tests in `crates/pcr-core/src/{projects,supabase}.rs` plus the
integration tests in `crates/pcr-cli/tests/golden.rs` pin most of this
down. If you add a schema migration, bump the version, write the up
SQL, and add a test for it.

## Output discipline

- **stderr** carries human-readable status, colors, and full-screen TUI
  frames.
- **stdout** carries machine-parseable JSON, and only when `--json` is
  set.
- Any command that defaults to a TUI **must** fall back to line output
  when `agent::is_tui_eligible(mode)` returns false. That helper checks
  TTY, `CI`, `NO_COLOR`, `CURSOR_AGENT`, `CURSOR_SANDBOX`, and the
  explicit `--plain` / `--json` flags — don't re-invent it.
- Respect `NO_COLOR` and `FORCE_COLOR` via `agent::colors_enabled`, not
  ad-hoc terminal probing.
- Plain-mode output is byte-stable: golden tests at
  `crates/pcr-cli/tests/golden.rs` will fail if you tweak it, even
  whitespace. Update the goldens when the change is intentional.

## TUI conventions

- Full-screen screens live in `crates/pcr-core/src/tui/screens/`.
  Reusable widgets in `crates/pcr-core/src/tui/widgets/`.
- Theme tokens are in `crates/pcr-core/src/tui/theme.rs` —
  `accent` (cyan), `success` (green), `pending` (yellow), `danger`
  (red), `dim` / `chrome` for layout. Don't hard-code RGBs in screens.
- The `HeaderBar` widget reads auth state via `auth::load()`. If you
  add a new screen, mirror the pattern in `tui/screens/status.rs` or
  `show.rs` — don't pass `user: None` or the header will lie about the
  signed-in state.
- Modal prompts (`Modal { kind, buf, targets, ... }` in
  `tui/screens/show.rs`) cancel on `Esc` or empty-buffer `q`. Confirm
  on `Enter`. Destructive modals (delete) use the danger accent and
  also confirm on `y`.
- Wrap any `Paragraph` that lives in a narrow column
  (`Wrap { trim: false }`); without it the text clips at the right
  border instead of wrapping.

## Source watchers

`crates/pcr-core/src/sources/{cursor,claude,vscode}/` each implement a
file-watcher that ingests the host editor's session transcripts. They
share helpers in `sources/shared/` (`git.rs`, `path_norm.rs`,
`tool_calls.rs`).

Invariants:

- Watchers run on the main thread inside `pcr start`'s tick loop. They
  must not block — long parsing should be debounced or pushed to a
  worker.
- New transcripts must produce the same `prompts.content_hash` whether
  captured live by `pcr start` or backfilled later by `cursor::force_sync`.
- Project attribution (`touched_project_ids`) is canonicalized via
  `path_norm::canonicalize_project_path_cache`. Don't bypass it — case
  sensitivity and `~` expansion both matter.

## Adding a dependency

- Add the crate with a pinned version to the workspace `Cargo.toml`
  under `[workspace.dependencies]`, then reference it from the member
  crate's `Cargo.toml` as `name = { workspace = true }`.
- **Avoid async runtimes** (`tokio`, `async-std`). The CLI is
  deliberately sync. The only exception is if MCP is being
  re-introduced via the `rmcp` crate.
- Avoid `libc` for portable ops; use `std::process`, `std::fs`,
  `std::env`, etc.
- Avoid C dependencies entirely — the build is offline-clean.
  `rusqlite` uses the bundled SQLite; `reqwest` uses `rustls-tls`.

## MCP

`crates/pcr-core/src/mcp/` is a stub returning `NotImplemented`. If
MCP is re-introduced, implement it against `rmcp` and delete the stub.
That is the one place where adding `tokio` is acceptable.

## Running locally

```bash
# Build the standalone binary used by Homebrew + GitHub Releases.
cargo build --release -p pcr-cli
./target/release/pcr --help

# Run the test suite (unit + integration + goldens).
cargo test --workspace

# Lint + format check matching CI.
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Build the .node addon for the npm distribution path.
cd crates/pcr-napi
npx @napi-rs/cli build --platform --release
```

CI runs `cargo fmt --check` as a hard gate — if `release.yml`'s `lint`
job fails, run `cargo fmt --all` locally and push the diff before
re-tagging.

## Releases

See `RELEASING.md`. tl;dr: bump the workspace version + every napi
sub-package's `package.json` to the same string, commit, tag with
`v0.X.Y`, push the tag. The release workflow handles npm + Homebrew +
GitHub Release artifact upload.
