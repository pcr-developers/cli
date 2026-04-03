# PCR.dev — Prompt Capture

Prompts are captured automatically by `pcr start`. After each Claude Code response, a Stop hook runs `pcr hook` which will ask you (in the terminal) whether to add new prompts to a bundle — press Y or N, no Claude involvement needed.

Use `pcr add`, `pcr commit`, and `pcr push` to manage bundles from the CLI.
