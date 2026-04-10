# PCR.dev — Human-First Prompt & Code Reviews

**The human-first platform for Prompt & Code Reviews in AI-native teams.**

If you're building software for real humans, humans should review the prompts powering that development. PCR.dev captures your AI prompt history from Cursor and Claude Code and surfaces it as a structured review artifact — by project, branch, and model — so your team can learn from each other's workflows and stay accountable for the AI decisions that shape what they ship.

PCRs (Prompt & Code Reviews) is a practice we coined and validated with 20 software engineers as part of peer-reviewed research at CHI 2026 (Best Paper Honorable Mention).

---

## Install

```bash
npm install -g pcr-dev
```

Also available via Homebrew (macOS):

```bash
brew tap pcr-developers/pcr
brew install pcr
```

Or download a standalone binary from [GitHub Releases](https://github.com/pcr-developers/cli/releases).

**Supported platforms:** macOS (Apple Silicon and Intel), Linux (x64)

---

## Quick start

```bash
pcr login          # opens your browser — create a CLI token in Settings and paste it back
cd your-project
pcr init           # registers the project and syncs it to your dashboard
pcr start          # watches Cursor and Claude Code sessions in the background
```

Then open [pcr.dev](https://pcr.dev). Your project appears and prompts start flowing in.

Full documentation at [pcr.dev/docs](https://pcr.dev/docs).

---

## What it captures

- **Cursor** — agent, ask, plan, and chat turns with file attribution
- **Claude Code** — full prompt/response exchanges including tool calls

Capture is passive. There is no clipboard access, no keylogging, and no modification to your AI tool. The CLI reads session files that your AI tool already writes to disk.

---

## Workflow

1. `pcr start` runs in the background and captures prompts as you work.
2. `pcr bundle "name" --select all` groups captured prompts into a named review artifact.
3. `pcr push` uploads the bundle to pcr.dev, where your team can review and comment.

---

## Open source

The prompt capture agent is open-source (Apache 2.0). The team platform at pcr.dev provides dashboards, review workflows, and analytics.

- [GitHub](https://github.com/pcr-developers/cli)
- [Issues](https://github.com/pcr-developers/cli/issues)
- [pcr.dev](https://pcr.dev)
