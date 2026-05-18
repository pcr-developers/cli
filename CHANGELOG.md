# Changelog

All notable changes to the `pcr-dev` CLI are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release line corresponds 1:1 to a git tag of the form `vX.Y.Z` and to
the `pcr-dev@X.Y.Z` npm dist-tag.

## [Unreleased]

## [0.3.0] — 2026-05-18

This release rolls up the in-flight watcher / dedup / packaging work
from PRs #85, #86, #87, and #88. The 0.3.0 minor bump (rather than
0.2.10) is motivated by the **public API change in pcr-core**:
`config::pcr_dir()` now returns `Result<PathBuf>` instead of `PathBuf`
(see #86). The CLI surface (`pcr <cmd>` shapes, flags, exit codes,
output format) is unchanged.

### Added

- **Update-available notice on `pcr` runs** ([#87]). Every interactive
  command now does a best-effort background check against the npm
  registry's `pcr-dev@latest` and prints a soft "X is available"
  notice at the end of the command if a newer version exists. The
  check is throttled (registry hit at most once per 24 h, notice
  shown at most once per 1 h), cached in
  `~/.pcr-dev/update-check.json`, and never blocks the command.
  Suggested upgrade command is install-method-aware:
  `brew upgrade pcr` for Homebrew installs, `npm i -g pcr-dev@latest`
  for npm installs. Opt out with `PCR_NO_UPDATE_CHECK=1` (also
  skipped automatically when `CI` is set, when `--json` is used, and
  for the internal `hook` and `mcp` subcommands).
- **`NOTICE` file** ([#88]) carrying the project's copyright
  assertion (`Copyright 2026 PCR.dev`) per Apache 2.0 §4(d).
- **`CHANGELOG.md`** ([#87]) — this file. Previously the CLI repo
  had no curated changelog; release notes lived only in commit
  messages and PR bodies.

### Changed

- **Public API:** `pcr_core::config::pcr_dir()` now returns
  `Result<PathBuf>` instead of `PathBuf` ([#86]). Callers that
  previously took a `PathBuf` from this function need to use `?` /
  `match` / `.ok()`. The runtime contract on a well-configured Unix
  or Windows machine is unchanged (the `Err` branch only fires
  inside sandboxes / containers where neither `$HOME` nor
  `%USERPROFILE%` resolves).
- **Cursor watcher periodic scan** ([#86]): interval bumped from
  20 s to 60 s. The `notify`-driven path is unchanged and still
  delivers ~600 ms end-to-end pickup latency; the periodic loop is
  now strictly a safety net for notify-misses.

### Fixed

- **VS Code Copilot Chat dual-watch dedup** ([#85]). When both
  `chatSessions/` and the legacy `transcripts/` directory exist on
  disk (the upgrade window for VS Code 0.45+), the watcher now
  scopes itself to `chatSessions/` only, so the same prompt is no
  longer ingested twice. Also normalises `captured_at` formatting in
  the V2 content hash so timestamp-only differences ("Z" vs
  "+00:00", millis vs micros) collapse to the same `prompt_id_v2`.
  Ships with a one-shot DB cleanup migration in the `functions` repo
  (`20260518000000_dedupe_vscode_prompts.sql`) for rows already
  double-written before this release.
- **`pcr start` graceful shutdown** ([#86]). The Ctrl-C handler was
  installed inside `wait_for_shutdown` — i.e. after PID-file write
  and after `spawn_all_sources`. SIGINT in that window killed the
  process with `W_TERMSIG(SIGINT)` and leaked the PID file. New
  `crate::shutdown` module wires a cooperative flag through every
  long-running scan loop (`cursor::watcher`,
  `session_state_watcher`, `diff_tracker`); the handler is installed
  at the very top of `commands::start::run` before any setup work.
- **`update_draft_response` overwrote multiple rows** ([#86]). The
  unscoped `UPDATE drafts WHERE session_id=? AND prompt_text=?` was
  overwriting every row with that `(session, prompt)` pair, so
  identical re-sent prompts ("go", "continue", "yes") had their
  earlier responses stomped by later turns. Now scoped to the single
  most recent matching row via `SELECT id ... LIMIT 1` + `UPDATE
  WHERE id = ?`.
- **`gc::orphaned` batched `git cat-file`** ([#86]). Was spawning one
  `git cat-file -e <sha>` per unpushed commit (O(N) processes per
  GC pass). Now a single `git cat-file --batch-check` invocation per
  repo: pipe every SHA on stdin, parse `<sha> missing` /
  `<sha> <type>` on stdout.
- **`pcr_dir()` no longer silently falls back to `/tmp`** ([#86]).
  Previously did `dirs::home_dir().unwrap_or_else(env::temp_dir)`,
  silently writing auth + SQLite + watcher state under `/tmp`
  whenever `$HOME` / `%USERPROFILE%` couldn't be resolved. Drafts
  and login evaporated on reboot. Now returns `anyhow::Result<PathBuf>`
  with a clear "set HOME and re-run" error.
- **Claude Code state cursor ordering** ([#86]). `state.set(file_path,
  lines)` ran before the parse — a transient parse failure (corrupt
  JSONL, mid-write truncation) advanced the cursor without saving
  anything, so unprocessed lines were lost until a full re-scan
  (which never happens in steady state). Moved `state.set` to after
  the `prompts.is_empty()` check.
- **Byte-slicing on IDs** ([#86]). Two spots (`commands/log.rs::short_sha`
  and `cursor::session_state_watcher` short-id log line) still
  byte-sliced on text we didn't allocate — composer IDs are UUIDs
  in practice but any future non-ASCII tag would have panicked.
  Switched to `chars().take(N).collect::<String>()`, matching the
  `truncate_diff` fix from #85.
- **Cursor watcher full-walk on every tick** ([#86]). The 20 s
  periodic `WalkDir` was CPU/disk-heavy at scale and re-processed
  every transcript on every pass. Now tracks top-level dir mtime
  plus a `notify_event_pending` flag; on the periodic tick, the walk
  is skipped entirely when neither signal indicates new activity.
  When a walk does run, per-file mtime cache skips `process_session`
  for paths whose mtime hasn't moved.
- **`LICENSE` no longer detects as "Other"** ([#88]). The previous
  file had dozens of substantive wording deviations from canonical
  Apache 2.0 (likely an old reformatter pass). Replaced with the
  verbatim `apache.org/licenses/LICENSE-2.0.txt` (11,358 bytes,
  byte-identical to upstream). GitHub now correctly reports the
  repo as Apache-2.0 in the sidebar and in the API. Existing
  `Cargo.toml` and `package.json` SPDX identifiers were already
  correct — no functional change to the license.

### Tests

- Workspace test count: 128 (v0.2.9 baseline) → **153** passing,
  0 failures. Net additions:
  - 11 new tests across #86's seven fixes (`start_graceful_shutdown`,
    `drafts_update_response_scoped`, `claudecode_state_ordering`,
    `gc::orphaned --batch-check` parsing, mtime-cache, `pcr_dir`
    error-branch, byte-slicing fallback).
  - 1 new test in #85 (`v2_hash_normalizes_captured_at_format`).
  - 6 new tests in #87 (`update_check::tests::*`: semver compare
    happy / prerelease / malformed, cache roundtrip, forward-compat
    cache deserialisation, quiet-subcommand skip).

[#85]: https://github.com/pcr-developers/cli/pull/85
[#86]: https://github.com/pcr-developers/cli/pull/86
[#87]: https://github.com/pcr-developers/cli/pull/87
[#88]: https://github.com/pcr-developers/cli/pull/88

## [0.2.9] — 2026-05-17

Last shipped release before this changelog file existed. See the
[v0.2.9 release notes][v029] on GitHub for the full diff. Highlights:

- TUI palette tokens + log goldens + parser edge tests + watcher cleanup
  (#83).
- TUI command-browser polish (#84).

Earlier releases pre-date this file. The full history is in the git log;
the most relevant ancestors are `v0.2.8`, `v0.2.7`, and `v0.2.0` (the
Rust port).

[v029]: https://github.com/pcr-developers/cli/releases/tag/v0.2.9
