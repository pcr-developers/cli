<div align="center">

# `pcr` — capture, bundle, and review every AI prompt behind your code

**The open-source CLI for [PCR.dev](https://pcr.dev) — Prompt & Code Review for AI-native teams.**

[![npm](https://img.shields.io/npm/v/pcr-dev?label=npm&color=cyan)](https://www.npmjs.com/package/pcr-dev)
[![Homebrew](https://img.shields.io/badge/homebrew-pcr--developers%2Fpcr-orange)](https://github.com/pcr-developers/homebrew-pcr)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue)](LICENSE)
[![CHI 2026](https://img.shields.io/badge/CHI%202026-Best%20Paper%20Honorable%20Mention-purple)](https://pcr.dev)

</div>

---

If you're shipping software for real humans, humans should review the prompts that
shaped that code. `pcr` runs in the background while you work, captures every
prompt you send to **Cursor**, **Claude Code**, and **VS Code Copilot**, attributes
each one to the right project, branch, and git SHA, and lets you bundle them into
named, reviewable artifacts that your team can comment on at
[pcr.dev](https://pcr.dev) — exactly like a code review, but for the prompts that
produced the diff.

We coined the practice of **Prompt & Code Reviews (PCRs)** and validated it with
20 software engineers in peer-reviewed research at CHI 2026 (Best Paper Honorable
Mention). This CLI is the capture-and-ship side of that practice.

> **Quick links:** [Install](#install) · [Two-minute tour](#two-minute-tour) ·
> [Commands](#commands) · [Privacy](#privacy) · [How capture works](#how-capture-works) ·
> [Use it inside an agent / CI](#agent--ci-friendly-output)

## Install

The CLI is one binary — no daemon, no editor extension, no background service.

```bash
# npm (any platform with Node ≥ 16)
npm install -g pcr-dev

# Homebrew (macOS, Linux)
brew tap pcr-developers/pcr
brew install pcr

# Or grab a prebuilt binary
# https://github.com/pcr-developers/cli/releases
```

**Supported platforms:** macOS Apple Silicon, macOS Intel, Linux x64, Windows x64.

> **Why npm works on locked-down Windows.** The npm package ships as a Rust-built
> Node native addon (`.node` file). It loads inside `node.exe`, which Windows
> AppLocker / WDAC already trusts — so you don't need admin rights, a code-signing
> certificate, or `winget` / Chocolatey to use `pcr` on a managed laptop.
> See [crates/pcr-napi/README.md](crates/pcr-napi/README.md) for the full story.

## Two-minute tour

```bash
pcr login                # opens your browser, paste a token from Settings
cd ~/code/your-repo
pcr init                 # registers this git repo as a tracked project

pcr start                # full-screen live dashboard — leave it running

# … work in Cursor / Claude Code / Copilot like you normally do …

pcr log                  # see drafts captured so far
pcr bundle "auth fix" --select 1-5
pcr push                 # ships the bundle to pcr.dev for review
```

That's it. Open [pcr.dev](https://pcr.dev) and your bundles are waiting for your team.

### What `pcr start` looks like

```text
PCR.dev · start · v0.2.2  ✓ bhada@pcr.dev                            14:08:42

  Capturing — 7 exchanges this session, 3 unbundled across 2 projects   ▁▃▆█▇▅▂

┌─ Watchers ──────────────────────────────────────────────────────────────────┐
│ ●  Cursor        ready     ~/.cursor/projects             fsnotify + scan   │
│ ●  Claude Code   ready     ~/.claude/projects             fsnotify + 1s     │
│ ◎  VS Code       waiting   …/User/workspaceStorage        waiting for tool  │
└─────────────────────────────────────────────────────────────────────────────┘
┌─ Projects · 5 registered · 3 unbundled · 1 bundle ──────────────────────────┐
│ ▸ ●  pcr-dev          main          5 · 0 · 1     5 unbundled               │
│   ●  cli              rust-port     2 · 1 · 0     3 unbundled               │
│   ○  functions        main          0 · 0 · 0     —                         │
└─────────────────────────────────────────────────────────────────────────────┘
┌─ Events · 7 captured this session ──────────────────────────────────────────┐
│ 14:08:33  ●  pcr-dev   "fix the diff viewer color contrast"   Write×2       │
│ 14:08:21  ●  cli       "rewrite display module to route via mpsc"           │
│ 14:07:50  ●  pcr-dev   "add EmptyState primitive to /projects"   Edit×3     │
│ 14:07:33  ◉  Cursor    ready                                                │
└─────────────────────────────────────────────────────────────────────────────┘
  ↑↓/jk project  v verbose  p pause  r refresh  q quit
```

Press `?` anywhere for keybinds.

## Commands

| Command | What it does |
|---|---|
| `pcr login` | Authenticate — opens your browser, paste a CLI token from Settings |
| `pcr logout` | Remove saved credentials |
| `pcr init` | Register the current git repo as a tracked project |
| `pcr start` | Live capture — full-screen TUI by default, `--plain` for line mode |
| `pcr status` | One-screen overview: auth, projects, draft → bundled → pushed pipeline |
| `pcr log` | Show captured prompts and bundles for this repo |
| `pcr show <n>` | Open one draft in the full-screen browser (number from `pcr log`) |
| `pcr bundle <name> --select <range>` | Group drafts into a named, reviewable bundle |
| `pcr push` | Ship sealed bundles to pcr.dev for review |
| `pcr pull <id>` | Restore a pushed bundle to local drafts (rare — for picking up a teammate's bundle) |
| `pcr gc` | Reclaim local-store space (delete pushed records, orphans, etc.) |
| `pcr help` | Browse every command interactively, with worked examples |

Run `pcr <command> --help` for a per-command write-up with examples and
cross-refs. The interactive `pcr help` reads from the same source-of-truth table,
so the two never drift.

## Privacy

Capture is **passive**. The CLI:

- Reads session files that Cursor / Claude Code / VS Code Copilot already
  write to disk on your machine.
- Does **not** touch your clipboard, log keystrokes, hook into your editor,
  or modify the AI tool in any way.
- Stores everything locally in `~/.pcr-dev/` until you explicitly run
  `pcr push`.

Until you push, every captured prompt lives only in your local SQLite store. You
can inspect it (`pcr log`, `pcr show`), edit which drafts go into a bundle
(`pcr bundle … --add / --remove`), or throw it away (`pcr gc --unpushed`) — all
without anything leaving your laptop.

## How capture works

```text
┌─────────────────┐     reads session files     ┌─────────────┐
│  Cursor         │ ─────────────────────────▶  │             │
│  Claude Code    │ ─────────────────────────▶  │  pcr start  │ ──▶ ~/.pcr-dev/drafts.db
│  Copilot Chat   │ ─────────────────────────▶  │             │           │
└─────────────────┘                              └─────────────┘           │
                                                                          │
                                                  pcr bundle "name" ──────┘
                                                          │
                                                          ▼
                                              pcr push   ──▶  pcr.dev
                                                                  │
                                                                  ▼
                                                          team review + comments
```

Per source we capture:

- **Cursor** — agent / ask / plan / chat turns, model, mode, tool calls,
  changed-file attribution from a periodic `git status` poll.
- **Claude Code** — full prompt/response exchanges including tool calls
  (Bash, Read, Edit, Write, Grep, etc.).
- **VS Code Copilot Chat** — JSONL transcripts from `workspaceStorage`,
  including sessions in unsaved windows.

Each draft is tagged with project, branch, git SHA at capture time, model,
permission mode, and (when available) the diff that the AI's edits produced.

## Agent / CI friendly output

`pcr` is built to work the same whether a human or an LLM is driving the
terminal:

- **`--plain`** disables the ratatui TUI for any subcommand. Output stays on
  stderr; stdout is reserved for machine-readable content.
- **`--json`** emits a structured JSON document on stdout (currently
  supported by `pcr status`; more commands rolling out).
- **Auto-detect**: if stdout/stderr isn't a TTY, or `CI`, `NO_COLOR`,
  `CURSOR_AGENT`, etc. is set, the CLI behaves as if you'd passed `--plain`.
- **Stable exit codes** —
  `0` success · `2` usage error · `10` auth required · `11` network · `13` not found ·
  `40` interactive unavailable · `50` not implemented.

This means you can wire `pcr` into a Cursor agent, a Claude Code subprocess,
or a CI step without it crashing on missing TTY assumptions.

## Workflow at a team level

1. Each engineer keeps `pcr start` running while they work.
2. After a meaningful unit of work — a feature, a fix, an experiment —
   they `pcr bundle "name"` it and `pcr push`.
3. Reviewers open the bundle on pcr.dev, scrub through the prompt history
   alongside the diff, and leave inline comments — exactly like a PR review.
4. The CLI stays out of the way until the next bundle.

## Repo layout

```text
cli/
├── crates/
│   ├── pcr-core/   ← all logic: capture, store, supabase, TUI, commands
│   ├── pcr-cli/    ← thin standalone-binary entry point (used by Homebrew)
│   └── pcr-napi/   ← Node-API bridge (used by `npm install -g pcr-dev`)
├── .github/workflows/release.yml
├── AGENTS.md       ← contribution guide for AI assistants and humans
├── RELEASING.md    ← how a release is cut end-to-end
└── README.md       ← you are here
```

## Building from source

You'll need Rust ≥ 1.82 (the `rust-toolchain.toml` pins the exact version).

```bash
git clone https://github.com/pcr-developers/cli.git
cd cli
cargo build --release -p pcr-cli         # standalone binary at target/release/pcr
cargo test --workspace                   # ~25 tests, runs in seconds

# Run without installing:
./target/release/pcr help
```

The Rust workspace has zero C dependencies — `rusqlite` uses the bundled
SQLite, `reqwest` uses `rustls-tls`. A clean checkout compiles offline.

## Contributing

Issues and PRs welcome. Please read [AGENTS.md](AGENTS.md) first — it codifies
the output discipline (line mode must stay byte-stable for golden tests),
the source-watcher invariants, and the conventions an AI assistant should
follow when modifying this codebase.

## License

Apache 2.0. See [LICENSE](LICENSE).

## Research

> Feng, Dana, Bhada Yun, and April Yi Wang.
> *"From Junior to Senior: Allocating Agency and Navigating Professional Growth
> in Agentic AI–Mediated Software Engineering."*
> **CHI 2026.** Best Paper Honorable Mention.

---

<div align="center">

[pcr.dev](https://pcr.dev) · [@pcr-developers](https://github.com/pcr-developers) · [npm](https://www.npmjs.com/package/pcr-dev)

</div>
