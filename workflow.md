# PCR.dev Workflow

## One-time setup

```bash
# 1. Register your project
cd /path/to/your/project
pcr init

# 2. Log in (opens pcr.dev/settings — create a CLI token there, paste it back)
pcr login
```

---

## Daily workflow

### 1. Start the watcher (keep running in a separate terminal)
```bash
pcr start
```
Watches all registered AI tool session directories and saves every prompt as a local draft automatically. Only captures prompts written **after** `pcr start` is running — nothing retroactive.

Currently supported: Cursor, Claude Code. New sources can be added by implementing the `CaptureSource` interface — see [CONTRIBUTING.md](CONTRIBUTING.md).

### 2. Work normally in your AI coding tool

Prompts are captured automatically by the watcher — no extra steps needed.

**If your tool supports a Stop hook** (currently: Claude Code), a prompt fires after each response:
```
PCR: 2 new prompts — add to "auth-refactor"? [Y/n/b]
```
- **Y** or **Enter** — add to the current open prompt bundle
- **n** — skip (prompts stay as drafts)
- **b** — branch into a new prompt bundle (you've switched tasks); prompts for a name

Single keypress, no Enter needed. Only fires when `pcr start` is running.

Open bundles created by the hook are automatically sealed when you run `pcr push` — no manual sealing step needed.

**If your tool doesn't have a Stop hook** (currently: Cursor, Codex, others), prompts are
saved as drafts automatically. Bundle them manually with `pcr bundle` after your session.

### 3. Check what's been captured
```bash
pcr status    # auth, projects, bundles, draft count at a glance
pcr log       # full history for the current repo
```

### 4. Create a prompt bundle

```bash
pcr bundle                                  # see all drafts (numbered) + unpushed bundles
pcr bundle "auth fix" --select 1-5          # bundle drafts 1-5 (auto-sealed, ready to push)
pcr bundle "auth fix" --select all          # bundle all drafts
pcr show 3                                  # see full text of draft #3 before bundling
```

**Edit a bundle before pushing:**
```bash
pcr bundle "auth fix" --add --select 6,7    # add more prompts
pcr bundle "auth fix" --remove --select 2   # remove a prompt (returns to drafts)
pcr bundle "auth fix" --delete              # delete the whole bundle
pcr bundle --list                           # see all unpushed bundles
```

### 5. Push to PCR.dev
```bash
pcr push
```
Seals any open bundles automatically, then pushes all sealed bundles. Each prompt is uploaded with:
- An incremental git diff scoped to the files the AI actually edited in that response
- Multi-repo attribution: if a response touched files in multiple registered repos, all of them are tagged on the prompt and bundle

Output:
```
PCR: Sealed "auth-refactor"
PCR: Pushed "auth refactor" (5 prompts)
    Branch:  feature/auth-refactor
    Review:  https://pcr.dev/review/<id>
    PR:      https://github.com/org/repo/pull/42
```

---

## All commands

| Command | What it does |
|---|---|
| `pcr status` | Auth state, registered projects, bundle summary, draft count |
| `pcr log` | Full prompt history for the current repo |
| `pcr start` | Start the file watcher (run in background terminal) |
| `pcr bundle` | Show all drafts (numbered) and unpushed bundles |
| `pcr bundle "name" --select 1-5` | Create prompt bundle from drafts 1-5 (auto-sealed) |
| `pcr bundle "name" --select all` | Bundle all drafts |
| `pcr bundle "name" --add --select 6,7` | Add more prompts to an existing bundle |
| `pcr bundle "name" --remove --select 2` | Remove a prompt from a bundle |
| `pcr bundle "name" --delete` | Delete a bundle (prompts return to drafts) |
| `pcr bundle --list` | List all unpushed bundles |
| `pcr show <number>` | Show full text of a specific draft |
| `pcr push` | Push all sealed bundles to PCR.dev |
| `pcr pull` | Restore a pushed bundle back to local drafts |
| `pcr gc --orphaned` | Remove bundles whose git SHA no longer exists |
| `pcr init` | Register current directory (or all sub-repos) as tracked projects |
| `pcr init --unregister` | Unregister the current project |
| `pcr login` | Authenticate with PCR.dev |
| `pcr logout` | Remove saved credentials |
| `pcr mcp` | Start the MCP server for MCP-compatible tools (called automatically) |
