<div align="center">

<img src="https://pcr.dev/og.png" alt="PCR.dev — Prompt & Code Reviews for AI-native teams" width="640" />

# PCR — Prompt & Code Review for AI-native engineering teams

### Capture every AI prompt behind every diff. Bundle them into reviewable artifacts. Discuss them like a code review.

[![npm](https://img.shields.io/npm/v/pcr-dev?label=npm&color=cyan&style=flat-square)](https://www.npmjs.com/package/pcr-dev)
[![Homebrew](https://img.shields.io/badge/homebrew-pcr--developers%2Fpcr-orange?style=flat-square)](https://github.com/pcr-developers/homebrew-pcr)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue?style=flat-square)](LICENSE)
[![CHI 2026](https://img.shields.io/badge/CHI%202026-Best%20Paper%20Honorable%20Mention-purple?style=flat-square)](https://pcr.dev/research)
[![Tests](https://img.shields.io/github/actions/workflow/status/pcr-developers/cli/release.yml?style=flat-square&label=ci)](https://github.com/pcr-developers/cli/actions)

[**pcr.dev**](https://pcr.dev) · [**Docs**](https://pcr.dev/docs) · [**Quickstart**](#-quickstart) · [**Why PCRs?**](#-why-prompt--code-reviews) · [**Research**](#-research)

</div>

---

## Table of contents

- [What is PCR.dev?](#-what-is-pcrdev)
- [Why Prompt & Code Reviews](#-why-prompt--code-reviews)
- [How it works](#-how-it-works)
- [What gets captured](#-what-gets-captured)
- [Quickstart](#-quickstart)
- [Install](#-install)
- [Commands](#-commands)
- [The live dashboard](#-the-live-dashboard)
- [Team workflow](#-team-workflow)
- [Privacy and security](#-privacy-and-security)
- [Use inside agents and CI](#-use-inside-agents-and-ci)
- [Frequently asked questions](#-frequently-asked-questions)
- [Build from source](#-build-from-source)
- [Contributing](#-contributing)
- [Research](#-research)
- [Community & support](#-community--support)
- [License](#-license)

---

## 🟢 What is PCR.dev?

**PCR.dev is the human review layer for AI-assisted software development.** When
your team ships code that an AI helped write, the diff alone doesn't tell the
whole story — the *prompts* that produced it carry the engineering judgment.
PCR captures those prompts, attributes them to the right project, branch, and
commit, and lets your team review them with the same rigor you give to a pull
request.

Concretely, PCR is two things:

1. **`pcr`** — an open-source CLI (this repo) that runs in the background on
   each engineer's laptop, watches their AI sessions in **Cursor**, **Claude
   Code**, and **VS Code Copilot Chat**, and stores prompts as local "drafts"
   you can later group and ship.
2. **[pcr.dev](https://pcr.dev)** — the team dashboard where bundled prompts
   become first-class review artifacts. Reviewers can leave inline comments on
   prompts the same way they comment on lines of code, see the diff each
   prompt produced, and link the bundle to a GitHub PR.

The combination is what we call a **Prompt & Code Review (PCR)** — a practice
we coined and validated in peer-reviewed research at CHI 2026 (Best Paper
Honorable Mention).

---

## 🧭 Why Prompt & Code Reviews

Code review answers *"is this diff correct?"* It can't answer:

- **Did we ask the right question?** A prompt that nudges the model toward a
  brittle one-off solution can produce a passing diff that nobody flags.
- **Is the AI being trusted with the right things?** A junior engineer letting
  the agent freely refactor `auth/` deserves a different conversation than
  one carefully sketching a single function.
- **What did we learn?** The prompt history *is* the rationale. Discarding
  it loses the "why" behind every commit.

PCRs make all of that visible. After a feature is shipped, reviewers see:

- Each prompt sent in chronological order, scoped to the right files.
- The diff each prompt produced, side by side with the prompt itself.
- Tool calls, model, mode (agent / ask / plan), permission level.
- Inline comments from teammates — exactly like GitHub PR review.

Senior engineers regain visibility into how AI is shaping their codebase.
Juniors get a traceable record of the decisions behind their work. Teams
build a shared library of *good prompts* the way they once built shared
libraries of good code.

---

## ⚙️ How it works

```text
   Cursor / Claude Code / VS Code Copilot
                   │
                   │  writes session files to disk (the AI tool already does this)
                   ▼
              ┌─────────┐                              ┌──────────────┐
              │  pcr    │   reads & attributes         │  ~/.pcr-dev/ │
              │  start  │ ───────────────────────────▶ │  drafts.db   │
              └─────────┘                              └──────────────┘
                                                             │
              ┌────────────────┐                             │
              │ pcr bundle ... │ ◀───────────────────────────┘
              └────────────────┘
                   │
                   │  group N drafts into one named artifact
                   ▼
              ┌─────────┐                ┌──────────────┐
              │ pcr push│ ──────────────▶│  pcr.dev     │ ◀── reviewers comment
              └─────────┘                └──────────────┘
```

Three steps from raw prompt to team review:

| Step | Command | What happens |
|---|---|---|
| **Capture** | `pcr start` | Foreground watcher reads session files your AI tool already writes. Each user prompt becomes a local `draft`. Zero impact on your editor — no extension, no plugin, no patches. |
| **Bundle** | `pcr bundle "auth fix" --select 1-5` | Group N coherent drafts into a sealed, named artifact. Bundles are mutable until pushed. |
| **Ship** | `pcr push` | Upload sealed bundles to [pcr.dev](https://pcr.dev) where your team reviews them. |

Everything lives only in `~/.pcr-dev/` until you explicitly run `pcr push`.

---

## 📥 What gets captured

| Source | Captured | How |
|---|---|---|
| **Cursor** | Agent / ask / plan / chat turns, model, mode, file attribution, periodic git diff for changed files | `~/.cursor/projects/` JSON sessions + 20s `git status --porcelain` poll |
| **Claude Code** | Full prompt + response, every tool call (Bash, Read, Edit, Write, Grep, …), permission mode, project | `~/.claude/projects/` JSONL transcripts via fsnotify |
| **VS Code Copilot Chat** | Prompt + response, tool calls, including chats in unsaved windows | `Library/Application Support/Code/User/workspaceStorage` JSONL |

Each captured exchange is annotated with:

- **Project** — registered git repo it was captured against (run `pcr init` in
  any repo to register it).
- **Branch & SHA** — the git ref the working tree was on at capture time.
- **Model & mode** — `claude-sonnet-4-6`, `agent`, `acceptEdits`, etc.
- **Diff** — when available, the actual changes the AI produced.

---

## 🚀 Quickstart

```bash
# 1. Install
npm install -g pcr-dev          # or: brew install pcr-developers/pcr/pcr

# 2. Sign in (opens your browser)
pcr login

# 3. Register a project
cd ~/code/your-repo
pcr init

# 4. Start capturing
pcr start                       # leave running while you work in your editor

# 5. Bundle and ship
pcr bundle "auth fix" --select all
pcr push                        # see the result at https://pcr.dev
```

Open [pcr.dev](https://pcr.dev) and your bundles are waiting for your team.

> **Need help mid-flow?** Run `pcr help` for an interactive command browser.
> Press `Enter` on any command to launch it directly.

---

## 📦 Install

| Platform | Method | Command |
|---|---|---|
| Any (with Node ≥ 16) | npm | `npm install -g pcr-dev` |
| macOS, Linux | Homebrew | `brew tap pcr-developers/pcr && brew install pcr` |
| Any | Standalone binary | [Download from Releases](https://github.com/pcr-developers/cli/releases/latest) |

**Supported platforms:** macOS Apple Silicon · macOS Intel · Linux x64 · Windows x64.

#### Why npm works on locked-down corporate Windows

The npm package ships as a Rust-built Node native addon (a `.node` file
loaded into `node.exe`), not a standalone `.exe`. This means it works under
Windows AppLocker / WDAC policies that would normally block unsigned
executables in user-writable directories — no admin rights, no code-signing
certificate, no `winget` or `Chocolatey` required. See
[`crates/pcr-napi/README.md`](crates/pcr-napi/README.md) for the technical
details.

---

## 🔧 Commands

| Command | What it does |
|---|---|
| [`pcr login`](https://pcr.dev/docs/login) | Authenticate — opens your browser for a CLI token |
| [`pcr logout`](https://pcr.dev/docs/logout) | Remove saved credentials |
| [`pcr init`](https://pcr.dev/docs/init) | Register the current git repo as a tracked project |
| [`pcr start`](https://pcr.dev/docs/start) | Live capture — full-screen TUI by default, `--plain` for line mode |
| [`pcr status`](https://pcr.dev/docs/status) | One-screen overview: auth, projects, draft → bundled → pushed pipeline |
| [`pcr log`](https://pcr.dev/docs/log) | Captured prompts and bundles for the current repo |
| [`pcr show <n>`](https://pcr.dev/docs/show) | Open one draft in the full-screen browser |
| [`pcr bundle <name>`](https://pcr.dev/docs/bundle) | Group drafts into a named, reviewable bundle |
| [`pcr push`](https://pcr.dev/docs/push) | Ship sealed bundles to pcr.dev |
| [`pcr pull <id>`](https://pcr.dev/docs/pull) | Restore a pushed bundle to local drafts |
| [`pcr gc`](https://pcr.dev/docs/gc) | Reclaim local-store space |
| [`pcr help`](https://pcr.dev/docs/help) | Interactive command browser — press `Enter` to launch any command |

Run `pcr <cmd> --help` for per-command examples and cross-references. Both
`--help` and the interactive `pcr help` read from the same source-of-truth
table, so the two never drift.

---

## 🖥 The live dashboard

`pcr start` runs a full-screen [ratatui](https://ratatui.rs) dashboard
showing capture sources, registered projects, the live event log, and a
draft-to-pushed pipeline at a glance:

```text
PCR.dev · start · v0.2.4  ✓ bhada@pcr.dev                            14:08:42

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
└─────────────────────────────────────────────────────────────────────────────┘
  ↑↓/jk project  v verbose  p pause  r refresh  q quit
```

Press `?` anywhere for keybinds. The TUI never bypasses your terminal — when
stdout is a pipe, `CI=1`, `NO_COLOR` is set, or you pass `--plain`, the same
information falls back to plain stderr lines.

---

## 👥 Team workflow

PCR is designed to fit the rhythm your team already has, not replace it.

1. **Each engineer keeps `pcr start` running** while they work. It sits in a
   background terminal and captures continuously — no clicks, no decisions.
2. **After a meaningful unit of work** — a feature, a fix, an experiment —
   they run `pcr bundle "name" --select <range>` and `pcr push`.
3. **Reviewers open the bundle on pcr.dev**, scrub through the prompt history
   alongside the diff, and leave inline comments — exactly like a PR review.
4. **(Optional) Connect GitHub** in [Settings](https://pcr.dev/settings) and
   the bundle auto-attaches to its corresponding pull request, surfacing the
   prompt history right next to the diff for whoever's reviewing.

The CLI stays out of the way until the next bundle. There's no "always-on"
agent, no daemon, no hook into your editor.

---

## 🔒 Privacy and security

PCR is **passive, local-first, and open**:

- ✅ Reads session files that Cursor / Claude Code / Copilot already write
  to disk on your machine. No clipboard access, no keylogging, no editor
  hooks, no patches to your AI tool.
- ✅ Stores everything locally in `~/.pcr-dev/` until you explicitly run
  `pcr push`. You can `pcr log`, `pcr show`, edit which drafts go into a
  bundle, or throw the lot away with `pcr gc --unpushed` — without anything
  leaving your laptop.
- ✅ Open source under [Apache 2.0](LICENSE) so you can read every line of
  the capture and ship logic.
- ✅ TLS-only push (`reqwest` + `rustls`), per-user CLI tokens, RLS-protected
  rows on the server side.
- ✅ No telemetry beyond what you explicitly push.

A push is opt-in, per-bundle. Until you type `pcr push`, your prompts never
leave your machine.

---

## 🤖 Use inside agents and CI

`pcr` is built to behave correctly whether a human or an LLM is driving the
terminal:

- **`--plain`** disables the ratatui TUI for any subcommand. Output stays on
  stderr; stdout is reserved for machine-readable content.
- **`--json`** emits structured JSON on stdout (currently `pcr status`; more
  rolling out).
- **Auto-detect** — if stdout/stderr isn't a TTY, or `CI`, `NO_COLOR`,
  `CURSOR_AGENT`, etc. is set, the CLI behaves as if you'd passed `--plain`
  without you having to set the flag.
- **Stable exit codes** — `0` success, `2` usage error, `10` auth required,
  `11` network, `13` not found, `40` interactive unavailable, `50` not
  implemented.

This means you can wire `pcr` into a Cursor agent, a Claude Code subprocess,
a GitHub Action, or a build script without it crashing on missing TTY
assumptions or printing escape codes into log aggregators.

```bash
# Common agent patterns:
pcr --json status                     # parse the pipeline state
pcr --plain log                       # get every prompt as plain text
pcr --plain bundle "agent run" --select all && pcr --plain push
```

---

## ❓ Frequently asked questions

<details>
<summary><b>Does PCR work without an internet connection?</b></summary>

Yes. Capture is fully local — `pcr start`, `pcr log`, `pcr show`, `pcr bundle`,
and `pcr gc` work offline. Only `pcr push`, `pcr pull`, and `pcr login`
need a network.
</details>

<details>
<summary><b>Can I use PCR without a pcr.dev account?</b></summary>

Yes. Without `pcr login`, prompts are still captured to local drafts. You
just can't `pcr push` them anywhere. This is useful for solo workflows or
for evaluating PCR before adopting it on a team.
</details>

<details>
<summary><b>Will PCR slow down my editor or AI tool?</b></summary>

No. PCR doesn't touch the editor process. It reads the session files
your AI tool *already* writes, via standard `fsnotify` watchers and a
periodic `git status --porcelain` poll. CPU is near-zero between events.
</details>

<details>
<summary><b>What about prompts I'd rather not share?</b></summary>

Drafts are local until you push them, and you choose which drafts go into
a bundle. Anything you don't `--select` stays on your machine. To purge,
run `pcr gc --unpushed`.
</details>

<details>
<summary><b>How does PCR handle multi-repo projects (monorepos, sub-modules)?</b></summary>

`pcr init` registers a single git repo. In a monorepo, run `pcr init` at
the root — every commit is attributed to the same project but carries the
correct branch + SHA + diff scope.
</details>

<details>
<summary><b>Can I self-host the dashboard?</b></summary>

The CLI is open source. The team dashboard at pcr.dev is currently a hosted
product. If you have a strong self-hosting requirement, [open an issue](https://github.com/pcr-developers/cli/issues)
or reach out at [hello@pcr.dev](mailto:hello@pcr.dev) — we're tracking
demand.
</details>

<details>
<summary><b>What languages and frameworks does PCR support?</b></summary>

PCR is language- and framework-agnostic. It captures prompts at the AI tool
level (Cursor / Claude Code / Copilot), so anything you can build with
those — TypeScript, Rust, Python, Go, Swift, Kotlin, SQL — is covered.
</details>

<details>
<summary><b>Does PCR work on Windows?</b></summary>

Yes — and unlike many AI-dev CLIs, it works on locked-down managed
Windows machines too, because it ships as a Node native addon rather than
a standalone executable. See the [npm distribution README](crates/pcr-napi/README.md)
for the technical reason.
</details>

---

## 🛠 Build from source

You'll need Rust ≥ 1.82 (the `rust-toolchain.toml` pins the exact version).

```bash
git clone https://github.com/pcr-developers/cli.git
cd cli
cargo build --release -p pcr-cli         # standalone binary at target/release/pcr
cargo test --workspace                   # ~25 tests, runs in seconds

./target/release/pcr help                # try it without installing
```

The Rust workspace has zero C dependencies — `rusqlite` uses the bundled
SQLite, `reqwest` uses `rustls-tls`. A clean checkout compiles offline.

#### Repo layout

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

---

## 🤝 Contributing

Issues and PRs welcome. Please read [AGENTS.md](AGENTS.md) first — it
codifies output discipline (line mode must stay byte-stable for golden
tests), source-watcher invariants, and the conventions an AI assistant
should follow when modifying this codebase.

Good first issues are tagged
[`good first issue`](https://github.com/pcr-developers/cli/labels/good%20first%20issue).
Architectural changes should start with a discussion thread before a PR.

---

## 📚 Research

PCR is grounded in peer-reviewed research at CHI 2026:

> Feng, Dana, Bhada Yun, and April Yi Wang. *"From Junior to Senior:
> Allocating Agency and Navigating Professional Growth in Agentic AI–Mediated
> Software Engineering."* **CHI 2026.** Best Paper Honorable Mention.

The paper introduces Prompt & Code Reviews as a formal software engineering
practice and evaluates it with 20 software engineers.

---

## 💬 Community & support

- **Issues** — [github.com/pcr-developers/cli/issues](https://github.com/pcr-developers/cli/issues)
- **Discussions** — [github.com/pcr-developers/cli/discussions](https://github.com/pcr-developers/cli/discussions)
- **Email** — [hello@pcr.dev](mailto:hello@pcr.dev)
- **Twitter/X** — [@pcrdev](https://twitter.com/pcrdev)

---

## 📜 License

[Apache 2.0](LICENSE). Use it freely in any project, commercial or otherwise.

---

<div align="center">

**Made for engineers who want to know what their AI is actually doing.**

[pcr.dev](https://pcr.dev) · [Docs](https://pcr.dev/docs) · [@pcr-developers](https://github.com/pcr-developers) · [npm](https://www.npmjs.com/package/pcr-dev)

</div>
