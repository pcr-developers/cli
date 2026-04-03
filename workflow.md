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
Watches `~/.claude/projects/` and `~/.cursor/projects/` and saves every prompt as a local draft automatically. Only captures prompts written **after** `pcr start` is running — nothing retroactive.

### 2. Work normally in Claude Code or Cursor

After each Claude Code response, a Stop hook fires and asks:
```
PCR: 2 new prompts — add to "auth-refactor"? [Y/n]
```
Press **Y** or **Enter** to add to the most recent open bundle, **N** to skip. Single keypress, no Enter needed. Only fires when `pcr start` is running.

### 3. Check what's been captured
```bash
pcr status    # auth, projects, bundles, draft count at a glance
pcr log       # full history for the current repo
```

### 4. Manage bundles and drafts
```bash
pcr add                    # browse bundles (numbered list), pick one to edit
pcr add "auth refactor"    # add drafts directly to a named bundle (creates if new)
pcr add --remove "auth refactor"   # remove prompts from a bundle
pcr add --delete           # permanently delete draft prompts
```

**Inside the bundle editor** (`pcr add` → pick a number):
- `a` — add more prompts
- `r` — remove prompts
- `n` — rename the bundle
- `d` — delete the bundle (prompts returned to drafts)
- `s` — seal it (ready to push)

**Other bundle edits:**
```bash
pcr commit --rename "auth refactor" "login fix"     # rename a bundle
```

### 5. Seal the bundle
```bash
pcr commit "auth refactor"   # seal a specific bundle
pcr commit                   # interactive — pick from open bundles
```

### 6. Push to PCR.dev
```bash
pcr push
```
Output:
```
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
| `pcr add` | Browse and edit bundles interactively |
| `pcr add "name"` | Add drafts to a named bundle |
| `pcr add --remove "name"` | Remove prompts from a bundle |
| `pcr add --delete` | Permanently delete draft prompts |
| `pcr commit "name"` | Seal a bundle |
| `pcr commit --rename "old" "new"` | Rename a bundle |
| `pcr push` | Push all sealed bundles to PCR.dev |
| `pcr pull` | Restore a pushed bundle back to local drafts |
| `pcr gc --orphaned` | Remove bundles whose git SHA no longer exists |
| `pcr init` | Register current directory as a tracked project |
| `pcr init --unregister` | Unregister the current project |
| `pcr login` | Authenticate with PCR.dev |
| `pcr logout` | Remove saved credentials |
| `pcr mcp` | Start the MCP server (Claude Code calls this automatically) |
