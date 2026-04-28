<div align="center">

# pcr — Prompt & Code Review CLI

Capture the prompts your team sends to AI coding assistants. Review them like a code review.

[![npm version](https://img.shields.io/npm/v/pcr-dev?style=flat-square&color=cb3837)](https://www.npmjs.com/package/pcr-dev)
[![Homebrew](https://img.shields.io/badge/homebrew-pcr-orange?style=flat-square)](https://github.com/pcr-developers/homebrew-pcr)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue?style=flat-square)](LICENSE)

[Website](https://pcr.dev) · [Docs](https://pcr.dev/docs) · [Quickstart](#quickstart) · [Discussions](https://github.com/pcr-developers/cli/discussions)

</div>

---

## What this is

`pcr` is a local CLI that watches the AI coding sessions you already have running
(**Cursor**, **Claude Code**, **VS Code Copilot Chat**), captures every prompt with the
right project / branch / commit / model attribution, and lets you bundle and push them
to [pcr.dev](https://pcr.dev) where your team reviews them alongside the diff.

The CLI in this repo is open source under Apache 2.0. The dashboard at
[pcr.dev](https://pcr.dev) is a hosted product; the CLI works against a local-only
store without it.

## Install

```bash
npm install -g pcr-dev                                 # any platform, Node ≥ 16
brew install pcr-developers/pcr/pcr                    # macOS · Linux
```

Standalone binaries for every supported platform are on the
[Releases](https://github.com/pcr-developers/cli/releases/latest) page.

Supported platforms: macOS arm64 · macOS x64 · Linux x64 · Windows x64.

## Quickstart

```bash
pcr login                                # one-time browser auth (skip for local-only use)
cd ~/code/your-repo
pcr init                                 # register the current git repo
pcr start                                # leave running while you work in your editor
pcr bundle "auth refactor"               # opens the TUI: pick drafts, press b → enter
pcr push                                 # upload to pcr.dev for team review
```

For per-command examples, run `pcr <cmd> --help` or browse [pcr.dev/docs](https://pcr.dev/docs).

## How it works

The supported AI tools each write session transcripts to disk as you work. `pcr start`
watches those files, attributes every prompt to the right project / branch / commit, and
saves them locally. Nothing leaves your machine until you explicitly run `pcr push`.

```text
Cursor / Claude Code / Copilot
       │  (writes session files to disk — already happens)
       ▼
   pcr start ─────────▶ ~/.pcr-dev/drafts.db
       │
       ▼
   pcr bundle "name"                  (TUI: select drafts, press b → enter)
       │
       ▼
   pcr push      ────▶ pcr.dev      (reviewers comment on prompts as they would on code)
```

## What gets captured

| Source | Captured |
|---|---|
| **Cursor** | Agent / ask / plan turns, model, mode, file attribution, periodic git diff |
| **Claude Code** | Full prompt + response, every tool call, permission mode, project |
| **VS Code Copilot Chat** | Prompt + response, tool calls, including unsaved-window chats |

Each prompt is annotated with project, branch, commit, model, mode, and (when available)
the diff it produced.

## Commands

| Command | What it does |
|---|---|
| `pcr login` / `pcr logout` | Authenticate / sign out |
| `pcr init` | Register the current git repo |
| `pcr start` | Live capture (TUI by default; `--plain` for line mode) |
| `pcr status` | Auth · projects · pipeline overview |
| `pcr log` | Captured prompts and bundles for the current repo |
| `pcr show <n>` | Open one draft in the full-screen browser |
| `pcr bundle [name]` | Open the interactive bundle browser (name pre-fills the modal) |
| `pcr push` | Upload sealed bundles to pcr.dev |
| `pcr pull <id>` | Restore a pushed bundle to local drafts |
| `pcr gc` | Reclaim local-store space |
| `pcr help` | Interactive command browser |

Run `pcr <cmd> --help` for examples. Full reference at [pcr.dev/docs](https://pcr.dev/docs).

## Live dashboard

`pcr start` runs a [ratatui](https://ratatui.rs) TUI showing every watcher's status, your
registered projects, the live event log, and the draft → bundled → pushed pipeline:

```text
PCR.dev · start · v0.2.7  ✓ bhada@pcr.dev                            14:08:42

  Capturing — 7 exchanges this session, 3 unbundled across 2 projects   ▁▃▆█▇▅▂

┌─ Watchers ──────────────────────────────────────────────────────────────────┐
│ ●  Cursor        ready     ~/.cursor/projects             fsnotify + scan   │
│ ●  Claude Code   ready     ~/.claude/projects             fsnotify + 1s     │
│ ◎  VS Code       waiting   …/User/workspaceStorage        waiting for tool  │
└─────────────────────────────────────────────────────────────────────────────┘
┌─ Events · 7 captured this session ──────────────────────────────────────────┐
│ 14:08:33  ●  pcr-dev   "fix the diff viewer color contrast"   Write×2       │
│ 14:08:21  ●  cli       "rewrite display module to route via mpsc"           │
│ 14:07:50  ●  pcr-dev   "add EmptyState primitive to /projects"   Edit×3     │
└─────────────────────────────────────────────────────────────────────────────┘
  ↑↓/jk project  v verbose  p pause  r refresh  q quit
```

Falls back to plain stderr lines when stdout isn't a TTY, `CI=1`, `NO_COLOR` is set, or
`--plain` is passed.

## CI / agent use

`pcr` is built to behave correctly whether a human or an LLM is at the terminal.

- `--plain` disables the TUI; output stays on stderr.
- `--json` emits machine-readable JSON on stdout.
- Auto-detects non-TTY stdio, `CI`, `NO_COLOR`, `CURSOR_AGENT`, etc., and behaves as if
  `--plain` were passed.
- Stable exit codes: `0` success · `2` usage · `10` auth required · `11` network ·
  `13` not found · `40` interactive unavailable · `50` not implemented.

```bash
pcr --json status
pcr --plain bundle "agent run" --select all && pcr --plain push   # plain mode keeps --select for scripts
```

## Privacy

- Everything lives in `~/.pcr-dev/` until you explicitly run `pcr push`.
- Reads session files Cursor / Claude Code / Copilot already write to disk. No clipboard
  access, no keylogging, no editor extension, no patches to your AI tool.
- TLS-only push (`reqwest` + `rustls`); per-user CLI tokens; row-level security on the
  server side.
- No telemetry beyond what you push.

## Build from source

Rust 1.82 or newer (MSRV). The dev `rust-toolchain.toml` pins a current
stable release; `rustup` will install it on demand. Zero C dependencies —
`rusqlite` uses the bundled SQLite, `reqwest` uses `rustls-tls`. A clean
checkout compiles offline.

```bash
git clone https://github.com/pcr-developers/cli.git
cd cli
cargo build --release -p pcr-cli
cargo test --workspace
./target/release/pcr help
```

The npm package ships as a Node native addon (a `.node` file loaded into `node.exe`)
rather than a standalone executable, so it works under Windows AppLocker / WDAC policies
that block unsigned executables in user-writable directories. See
[`crates/pcr-napi/NPM-INTERNALS.md`](crates/pcr-napi/NPM-INTERNALS.md) for the technical
details.

### Repository layout

```text
cli/
├── crates/
│   ├── pcr-core/   # all logic: capture, store, supabase, TUI, commands
│   ├── pcr-cli/    # standalone-binary entry point (Homebrew + Releases)
│   └── pcr-napi/   # Node-API bridge (used by `npm install -g pcr-dev`)
├── .github/workflows/release.yml
├── AGENTS.md       # contribution guide for AI assistants and humans
├── RELEASING.md    # how a release is cut end-to-end
└── README.md
```

## Contributing

Issues and PRs welcome. Please read [AGENTS.md](AGENTS.md) first — it documents the
line-mode output discipline (must stay byte-stable for golden tests), source-watcher
invariants, and the conventions for both human and AI contributors.

Good first issues are tagged
[`good first issue`](https://github.com/pcr-developers/cli/labels/good%20first%20issue).
Architectural changes should start with a
[discussion thread](https://github.com/pcr-developers/cli/discussions) before a PR.

## Support

- **Issues:** [github.com/pcr-developers/cli/issues](https://github.com/pcr-developers/cli/issues)
- **Discussions:** [github.com/pcr-developers/cli/discussions](https://github.com/pcr-developers/cli/discussions)
- **Email:** [bhayun@ethz.ch](mailto:bhayun@ethz.ch)

## License

[Apache 2.0](LICENSE).
