/**
 * Watches ~/.cursor/projects/ for Cursor agent-transcript JSONL files.
 * Only processes files from projects registered via `pcr init`.
 *
 * Path pattern:
 *   ~/.cursor/projects/<project-slug>/agent-transcripts/<uuid>/<uuid>.jsonl
 */

import chokidar from "chokidar";
import { readFileSync, writeFileSync, existsSync, mkdirSync, type Stats } from "fs";
import { execSync } from "child_process";
import { basename, dirname, join } from "path";
import { homedir } from "os";
import { parseCursorTranscript } from "./cursor-parser.js";
import { insertPromptsBatch, upsertCursorSession, PromptRecord } from "../lib/supabase.js";
import { getRegisteredCursorSlugs, getProjectIdForCursorSlug } from "../lib/projects.js";
import { PCR_DIR } from "../lib/constants.js";
import { getSessionMeta, getFullSessionData } from "./cursor-db.js";
import { getProjectPathForCursorSlug } from "../lib/projects.js";
import { getCaptureProvenance } from "../lib/versions.js";

const STATE_FILE = join(homedir(), PCR_DIR, "cursor-state.json");

/**
 * Find all git commits made in the project while the Cursor session was open.
 * Uses `session_created_at` and `session_updated_at` from composerData as the
 * time window. Falls back gracefully if git is unavailable or timestamps are missing.
 */
function getCommitRange(
  projectPath: string,
  since?: number,
  until?: number
): { start: string | null; end: string | null; all: string[] } {
  try {
    const args = ["git", "log", "--format=%H", "--no-merges"];
    if (since) args.push(`--after=${new Date(since).toISOString()}`);
    if (until) args.push(`--before=${new Date(until).toISOString()}`);
    const output = execSync(args.join(" "), {
      cwd: projectPath,
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
      timeout: 5000,
    }).trim();
    const shas = output ? output.split("\n").filter(Boolean) : [];
    return {
      start: shas[shas.length - 1] ?? null,  // oldest
      end:   shas[0] ?? null,                  // newest
      all:   shas,
    };
  } catch {
    return { start: null, end: null, all: [] };
  }
}

function loadState(): Map<string, number> {
  try {
    if (existsSync(STATE_FILE)) {
      const raw = JSON.parse(readFileSync(STATE_FILE, "utf-8")) as Record<string, number>;
      return new Map(Object.entries(raw));
    }
  } catch {
    // Start fresh
  }
  return new Map();
}

function saveState(state: Map<string, number>): void {
  try {
    const dir = dirname(STATE_FILE);
    if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
    writeFileSync(STATE_FILE, JSON.stringify(Object.fromEntries(state), null, 2));
  } catch {
    // Non-fatal
  }
}

const fileLineCount = loadState();
const sentHashes = new Set<string>();

function hashPrompt(sessionId: string, promptText: string): string {
  return `${sessionId}:${promptText.slice(0, 100)}`;
}

function parseTranscriptPath(filePath: string): { projectSlug: string; sessionUuid: string } | null {
  const parts = filePath.split("/");
  const agentIdx = parts.indexOf("agent-transcripts");
  if (agentIdx < 2) return null;
  return {
    projectSlug: parts[agentIdx - 1],
    sessionUuid: basename(filePath, ".jsonl"),
  };
}

async function processFile(
  filePath: string,
  allowedSlugs: Set<string>,
  userId?: string,
  forceFullScan = false
): Promise<number> {
  try {
    const meta = parseTranscriptPath(filePath);
    if (!meta) return 0;

    // Only process files from registered projects
    if (allowedSlugs.size > 0 && !allowedSlugs.has(meta.projectSlug)) return 0;

    const content = readFileSync(filePath, "utf-8");
    const lines = content.trim().split("\n").filter((l) => l.trim());
    // During startup full-scan we treat previousCount as 0 so all lines are
    // re-processed. The content_hash upsert makes this idempotent — no actual
    // DB changes if the prompts are already there.
    const previousCount = forceFullScan ? 0 : (fileLineCount.get(filePath) || 0);

    if (lines.length <= previousCount) return 0;

    fileLineCount.set(filePath, lines.length);
    saveState(fileLineCount);

    const session = parseCursorTranscript(content, meta.sessionUuid, meta.projectSlug);

    const projectId = getProjectIdForCursorSlug(meta.projectSlug);

    // Enrich with metadata from Cursor's SQLite database
    const sessionMeta = getSessionMeta(meta.sessionUuid);

    // Build a map from "user turn index" → assistant bubble metadata
    // so we can attach response timing and relevant files per prompt
    const assistantBubbles = sessionMeta?.bubbles.filter((b) => b.type === 2) ?? [];

    const newPrompts: PromptRecord[] = [];
    let promptIndex = 0;
    for (const prompt of session.prompts) {
      const hash = hashPrompt(prompt.session_id, prompt.prompt_text);
      if (!sentHashes.has(hash)) {
        sentHashes.add(hash);

        const assistantBubble = assistantBubbles[promptIndex];

        // Provenance — cached after first call, no perf cost per prompt
        const { cursor_version, pcr_version, capture_schema } = getCaptureProvenance();
        const fileContext: Record<string, unknown> = {
          capture_schema,
          pcr_version,
          ...(cursor_version ? { cursor_version } : {}),
        };

        // is_agentic: per-bubble (accurate per-turn) when available,
        // fall back to session-level isAgentic.
        if (assistantBubble?.isAgentic !== undefined) {
          fileContext.is_agentic = assistantBubble.isAgentic;
        } else if (sessionMeta?.isAgentic !== undefined) {
          fileContext.is_agentic = sessionMeta.isAgentic;
        }
        // cursor_mode: session-level — reliable for dedicated plan/debug sessions.
        // Used by the badge to show "plan" vs "ask" vs "agent".
        if (sessionMeta?.unifiedMode) {
          fileContext.cursor_mode = sessionMeta.unifiedMode;
        }
        if (assistantBubble?.responseDurationMs) {
          fileContext.response_duration_ms = assistantBubble.responseDurationMs;
        }
        if (assistantBubble?.relevantFiles?.length) {
          fileContext.relevant_files = assistantBubble.relevantFiles;
        }
        // Per-turn code change data (old Cursor format _v < 14 only)
        if (assistantBubble?.diffHistories?.length) {
          fileContext.diff_histories = assistantBubble.diffHistories;
        }
        if (assistantBubble?.humanChanges?.length) {
          fileContext.human_changes = assistantBubble.humanChanges;
        }
        if (assistantBubble?.fileDiffTrajectories?.length) {
          fileContext.file_diff_trajectories = assistantBubble.fileDiffTrajectories;
        }

        // Model from session meta (available in Cursor _v >= 14)
        const modelFromDb = sessionMeta?.modelName;

        // Use the actual submission time from the DB if available.
        const capturedAt = assistantBubble?.submittedAt
          ? new Date(assistantBubble.submittedAt).toISOString()
          : prompt.captured_at;

        newPrompts.push({
          ...prompt,
          captured_at: capturedAt,
          // Set model from DB — the upsert RPC won't overwrite if user has
          // already manually tagged a different model (coalesce in SQL).
          model: modelFromDb ?? prompt.model,
          user_id: userId,
          project_id: projectId,
          file_context: Object.keys(fileContext).length > 0 ? fileContext : undefined,
        });

        promptIndex++;
      }
    }

    if (newPrompts.length === 0) return 0;

    const inserted = await insertPromptsBatch(newPrompts);
    if (inserted > 0) {
      console.error(`PCR [cursor]: Captured ${inserted} prompt(s) from ${meta.projectSlug}`);
    }

    // Upsert session metadata (once per file, not once per prompt).
    const fullSession = getFullSessionData(meta.sessionUuid);
    if (fullSession) {
      // Attach git commits made during this session's time window
      const projectPath = getProjectPathForCursorSlug(meta.projectSlug);
      if (projectPath) {
        const commits = getCommitRange(
          projectPath,
          fullSession.sessionCreatedAt,
          fullSession.sessionUpdatedAt
        );
        fullSession.commitShaStart = commits.start ?? undefined;
        fullSession.commitShaEnd   = commits.end   ?? undefined;
        fullSession.commitShas     = commits.all.length > 0 ? commits.all : undefined;
      }
      await upsertCursorSession(fullSession, projectId, userId);
    }

    return inserted;
  } catch {
    return 0;
  }
}

export function startCursorWatcher(
  cursorProjectsDir: string,
  userId: string | undefined
): void {
  // Re-read slugs dynamically in case new projects are registered while running
  const getAllowedSlugs = () => getRegisteredCursorSlugs();

  if (!existsSync(cursorProjectsDir)) {
    console.error(
      `PCR [cursor]: Projects directory not found at ${cursorProjectsDir}. Will activate when it appears.`
    );
  }

  console.error(`PCR [cursor]: Watching Cursor transcripts at ${cursorProjectsDir}`);

  let totalCaptured = 0;
  // True during the initial file discovery pass at startup.
  // All files are processed from scratch so any prompts missed while the
  // watcher was stopped are picked up. The content_hash upsert makes this
  // safe — already-stored prompts are a no-op.
  let initialScan = true;

  const watcher = chokidar.watch(cursorProjectsDir, {
    persistent: true,
    ignoreInitial: false,
    ignored: (filePath: string, stats?: Stats) =>
      stats?.isFile() === true && !filePath.endsWith(".jsonl"),
    awaitWriteFinish: {
      stabilityThreshold: 1500,
      pollInterval: 200,
    },
  });

  // "ready" fires once chokidar has finished the initial scan
  watcher.on("ready", () => {
    initialScan = false;
    console.error(`PCR [cursor]: Initial scan complete.`);
  });

  watcher.on("add", async (filePath) => {
    if (!filePath.endsWith(".jsonl")) return;
    if (!filePath.includes("/agent-transcripts/")) return;
    if (filePath.includes("/subagents/")) return;
    const count = await processFile(filePath, getAllowedSlugs(), userId, initialScan);
    if (count > 0) {
      totalCaptured += count;
      console.error(`PCR [cursor]: Total captured: ${totalCaptured}`);
    }
  });

  watcher.on("change", async (filePath) => {
    if (!filePath.endsWith(".jsonl")) return;
    if (!filePath.includes("/agent-transcripts/")) return;
    if (filePath.includes("/subagents/")) return;
    const count = await processFile(filePath, getAllowedSlugs(), userId);
    if (count > 0) {
      totalCaptured += count;
      console.error(`PCR [cursor]: Total captured: ${totalCaptured}`);
    }
  });

  watcher.on("error", (error) => {
    console.error("PCR [cursor]: Watcher error:", error);
  });
}
