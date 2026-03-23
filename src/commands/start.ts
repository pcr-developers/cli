import { homedir } from "os";
import { join } from "path";
import { existsSync, readFileSync, writeFileSync, unlinkSync, mkdirSync } from "fs";
import * as readline from "readline/promises";
import { loadAuth } from "../lib/auth.js";
import { loadProjects } from "../lib/projects.js";
import { startCursorWatcher } from "../watchers/cursor.js";
import { startClaudeCodeWatcher } from "../watchers/claude-code.js";
import { PCR_DIR } from "../lib/constants.js";
import { getCaptureProvenance } from "../lib/versions.js";

const PID_FILE = join(homedir(), PCR_DIR, "watcher.pid");

function readExistingPid(): number | null {
  try {
    if (!existsSync(PID_FILE)) return null;
    const pid = parseInt(readFileSync(PID_FILE, "utf-8").trim(), 10);
    if (isNaN(pid)) return null;
    // Check if process is actually running
    process.kill(pid, 0);
    return pid;
  } catch {
    // Process not running — stale PID file, clean it up
    try { unlinkSync(PID_FILE); } catch { /* ignore */ }
    return null;
  }
}

function writePid(): void {
  try {
    const dir = join(homedir(), PCR_DIR);
    if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
    writeFileSync(PID_FILE, String(process.pid));
  } catch { /* non-fatal */ }
}

function clearPid(): void {
  try { unlinkSync(PID_FILE); } catch { /* ignore */ }
}

export async function runStart(): Promise<void> {
  // ── Check for existing watcher ──────────────────────────────────────────
  const existingPid = readExistingPid();

  if (existingPid !== null) {
    console.error(`\nPCR: A watcher is already running (PID ${existingPid}).`);
    console.error("      Running multiple watchers wastes resources — dedup is");
    console.error("      handled by the database so they'd capture the same prompts.\n");

    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stderr,
    });

    const answer = (await rl.question("  Replace it? [y/N] ")).trim().toLowerCase();
    rl.close();

    if (answer !== "y" && answer !== "yes") {
      console.error("\nPCR: Keeping existing watcher. Exiting.\n");
      process.exit(0);
    }

    // Kill the old watcher
    try {
      process.kill(existingPid, "SIGTERM");
      console.error(`PCR: Stopped previous watcher (PID ${existingPid}).`);
    } catch {
      console.error(`PCR: Could not stop PID ${existingPid} — it may have already exited.`);
    }
    // Brief pause to let it clean up
    await new Promise((r) => setTimeout(r, 500));
  }

  // ── Auth + project info ─────────────────────────────────────────────────
  const auth = loadAuth();
  const projects = loadProjects();

  if (!auth) {
    console.error("PCR: Not logged in. Run `pcr login` first.");
  } else {
    console.error(`PCR: Authenticated as ${auth.userId}`);
  }

  if (projects.length === 0) {
    console.error(
      "PCR: No projects registered. Run `pcr init` in a project directory first."
    );
    console.error("PCR: Watcher will not capture any prompts until a project is initialized.");
  } else {
    console.error(`PCR: Watching ${projects.length} registered project(s):`);
    for (const p of projects) {
      const syncStatus = p.projectId
        ? "synced"
        : "local only — run `pcr init` after `pcr login` to sync";
      console.error(`  - ${p.name} (${p.path}) [${syncStatus}]`);
    }
  }

  // ── Write PID and start watchers ────────────────────────────────────────
  writePid();

  // Detect and log provenance metadata once at startup
  const provenance = getCaptureProvenance();
  console.error(`PCR: Capture schema v${provenance.capture_schema} · pcr-dev v${provenance.pcr_version}`);

  const userId = auth?.userId;
  const cursorDir = join(homedir(), ".cursor", "projects");
  const claudeDir = join(homedir(), ".claude", "projects");

  startClaudeCodeWatcher(claudeDir, userId);
  startCursorWatcher(cursorDir, userId);

  console.error("PCR: Watcher running. Press Ctrl+C to stop.");

  const shutdown = () => {
    console.error("\nPCR: Shutting down.");
    clearPid();
    process.exit(0);
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);

  // Keep process alive
  await new Promise(() => {});
}
