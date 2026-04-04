# PCR.dev — Prompt Capture

Prompts are captured automatically by `pcr start`. After each Claude Code response, a Stop hook runs `pcr hook` which will ask you (in the terminal) whether to add new prompts to a bundle — press Y, N, or B (branch into a new bundle). No Claude involvement needed.

Use `pcr bundle`, and `pcr push` to manage prompt bundles from the CLI.

Key commands:
- `pcr bundle` — see all drafts and unpushed bundles
- `pcr bundle "name" --select 1-5` — create a prompt bundle from drafts 1-5
- `pcr bundle "name" --select all` — bundle all drafts
- `pcr push` — push all sealed bundles to PCR.dev
