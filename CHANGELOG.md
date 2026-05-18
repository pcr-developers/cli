# Changelog

All notable changes to the `pcr-dev` CLI are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release line corresponds 1:1 to a git tag of the form `vX.Y.Z` and to
the `pcr-dev@X.Y.Z` npm dist-tag.

## [Unreleased]

### Added

- **Update-available notice on `pcr` runs.** Every interactive command
  now does a best-effort background check against the npm registry's
  `pcr-dev@latest` and prints a soft "X is available" notice at the end
  of the command if a newer version exists. The check is throttled
  (registry hit at most once per 24h, notice shown at most once per 1h),
  cached in `~/.pcr-dev/update-check.json`, and never blocks the
  command. Suggested upgrade command is install-method-aware:
  `brew upgrade pcr` for Homebrew installs, `npm i -g pcr-dev@latest`
  for npm installs. Opt out with `PCR_NO_UPDATE_CHECK=1` (also skipped
  automatically when `CI` is set, when `--json` is used, and for the
  internal `hook` and `mcp` subcommands).

### Fixed

The following items are in-flight as separate PRs and will be folded
into the next release when those land:

- **#85 — VS Code Copilot Chat dual-watch dedup.** When both
  `chatSessions/` and the legacy `transcripts/` directory exist on
  disk (the upgrade window for VS Code 0.45+), the watcher now scopes
  itself to `chatSessions/` only, so the same prompt is no longer
  ingested twice. Also normalises `captured_at` formatting in the V2
  content hash so timestamp-only differences ("Z" vs "+00:00", millis
  vs micros) collapse to the same `prompt_id_v2`. Ships with a
  one-shot DB cleanup migration in the `functions` repo
  (`20260518000000_dedupe_vscode_prompts.sql`).
- **#86 — Watcher correctness + perf.** Seven fixes from the recent
  audit:
  - `pcr start` Ctrl-C now triggers cooperative shutdown across every
    long-running scan loop (cursor watcher, session-state watcher,
    diff tracker) — was previously SIGINT-with-leaked-PID-file.
  - `update_draft_response` is scoped to the single most recent row
    matching `(session_id, prompt_text)`, fixing the silent overwrite
    when the same prompt text re-appears in one session ("go",
    "continue", "yes").
  - Cursor watcher walks are skipped entirely when neither the
    top-level dir mtime nor the notify-pending flag indicate new
    activity, with per-file mtime cache to skip already-parsed files
    on the cold path. Periodic interval bumped 20 s → 60 s.
  - `gc::orphaned` batches its existence checks into a single
    `git cat-file --batch-check` per repo, replacing O(N) one-shot
    subprocesses.
  - `pcr_dir()` now returns `Result<PathBuf>` and surfaces a clear
    "set HOME and re-run" error instead of silently writing auth +
    SQLite to `/tmp` when no home directory can be resolved.
  - Claude Code state cursor advances only after a successful parse,
    so a transient JSONL truncation no longer drops unprocessed
    lines.
  - Byte-slicing on user-supplied IDs replaced with `chars().take(N)`
    in `commands/log.rs::short_sha` and the session-state-watcher
    short-id log line — matches the `truncate_diff` fix from #85.

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
