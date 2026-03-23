/**
 * Watches ~/.claude/projects/ for Claude Code session JSONL files.
 * Only processes files from projects registered via `pcr init`.
 *
 * Path pattern:
 *   ~/.claude/projects/<project-slug>/<session-id>.jsonl
 *
 * Claude Code also supports native binary installation (not just npm).
 * The projects directory location is the same regardless of install method.
 */

import chokidar from "chokidar";
import { readFileSync, writeFileSync, existsSync, mkdirSync, type Stats } from "fs";
import { basename, dirname, join } from "path";
import { homedir } from "os";
import { parseClaudeCodeSession } from "./claude-code-parser.js";
import { insertPromptsBatch, PromptRecord } from "../lib/supabase.js";
import { getRegisteredClaudeSlugs, getProjectIdForClaudeSlug } from "../lib/projects.js";
import { PCR_DIR } from "../lib/constants.js";
import { getCaptureProvenance } from "../lib/versions.js";

const STATE_FILE = join(homedir(), PCR_DIR, "claude-state.json");

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

async function processFile(
  filePath: string,
  allowedSlugs: Set<string>,
  userId?: string
): Promise<number> {
  try {
    // Extract project slug from path:
    // ~/.claude/projects/<slug>/<session>.jsonl
    const parts = filePath.split("/");
    const projectsIdx = parts.lastIndexOf("projects");
    if (projectsIdx < 0 || projectsIdx + 1 >= parts.length) return 0;

    const projectSlug = parts[projectsIdx + 1];
    const projectName = basename(dirname(filePath)); // same as slug

    // Only process files from registered projects
    if (allowedSlugs.size > 0 && !allowedSlugs.has(projectSlug)) return 0;

    const content = readFileSync(filePath, "utf-8");
    const lines = content.trim().split("\n").filter((l) => l.trim());
    const previousCount = fileLineCount.get(filePath) || 0;

    if (lines.length <= previousCount) return 0;

    fileLineCount.set(filePath, lines.length);
    saveState(fileLineCount);

    const session = parseClaudeCodeSession(content, projectName, filePath);

    const projectId = getProjectIdForClaudeSlug(projectSlug);

    const { cursor_version, pcr_version, capture_schema } = getCaptureProvenance();
    const baseFileContext: Record<string, unknown> = {
      capture_schema,
      pcr_version,
      ...(cursor_version ? { cursor_version } : {}),
    };

    const newPrompts: PromptRecord[] = [];
    for (const prompt of session.prompts) {
      const hash = hashPrompt(prompt.session_id, prompt.prompt_text);
      if (!sentHashes.has(hash)) {
        sentHashes.add(hash);
        newPrompts.push({
          ...prompt,
          user_id: userId,
          project_id: projectId,
          file_context: { ...baseFileContext, ...(prompt.file_context ?? {}) },
        });
      }
    }

    if (newPrompts.length === 0) return 0;

    const inserted = await insertPromptsBatch(newPrompts);
    if (inserted > 0) {
      console.error(`PCR [claude]: Captured ${inserted} prompt(s) from ${projectName}`);
    }
    return inserted;
  } catch {
    return 0;
  }
}

export function startClaudeCodeWatcher(
  claudeCodeDir: string,
  userId: string | undefined
): void {
  const getAllowedSlugs = () => getRegisteredClaudeSlugs();

  if (!existsSync(claudeCodeDir)) {
    console.error(
      `PCR [claude]: Projects directory not found at ${claudeCodeDir}. Will activate when it appears.`
    );
  }

  console.error(`PCR [claude]: Watching Claude Code sessions at ${claudeCodeDir}`);

  let totalCaptured = 0;

  const watcher = chokidar.watch(claudeCodeDir, {
    persistent: true,
    ignoreInitial: false,
    ignored: (filePath: string, stats?: Stats) =>
      stats?.isFile() === true && !filePath.endsWith(".jsonl"),
    awaitWriteFinish: {
      stabilityThreshold: 1000,
      pollInterval: 200,
    },
  });

  watcher.on("add", async (filePath) => {
    if (!filePath.endsWith(".jsonl")) return;
    const count = await processFile(filePath, getAllowedSlugs(), userId);
    if (count > 0) {
      totalCaptured += count;
      console.error(`PCR [claude]: Total captured: ${totalCaptured}`);
    }
  });

  watcher.on("change", async (filePath) => {
    if (!filePath.endsWith(".jsonl")) return;
    const count = await processFile(filePath, getAllowedSlugs(), userId);
    if (count > 0) {
      totalCaptured += count;
      console.error(`PCR [claude]: Total captured: ${totalCaptured}`);
    }
  });

  watcher.on("error", (error) => {
    console.error("PCR [claude]: Watcher error:", error);
  });
}
