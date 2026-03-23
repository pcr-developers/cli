/**
 * Provenance metadata for captures.
 *
 * Stored in file_context for every captured prompt so we can track:
 *   - cursor_version: which Cursor release was running
 *   - pcr_version: which pcr-dev release captured it
 *   - capture_schema: integer version of our parsing format
 *
 * capture_schema must be manually incremented here whenever we change
 * how we read Cursor's JSONL files or SQLite database. This lets anyone
 * query "all prompts captured with the old format" and re-process them
 * when Cursor changes its storage format.
 *
 * History:
 *   1 — initial format (Cursor JSONL + composerData conversation array)
 *   2 — Cursor _v 14: conversation array replaced by fullConversationHeadersOnly;
 *         modelConfig.modelName now available; per-bubble timing/isAgentic not available
 */

import { existsSync, readFileSync } from "fs";
import { join } from "path";
import { platform } from "os";
import { execSync } from "child_process";
import { createRequire } from "module";

const require = createRequire(import.meta.url);

export const CAPTURE_SCHEMA_VERSION = 2;

// ---------------------------------------------------------------------------
// PCR version — read once from package.json at module load time
// ---------------------------------------------------------------------------

let _pcrVersion: string | null = null;

export function getPcrVersion(): string {
  if (_pcrVersion) return _pcrVersion;
  try {
    const pkg = require("../../package.json") as { version?: string };
    _pcrVersion = pkg.version ?? "unknown";
  } catch {
    _pcrVersion = "unknown";
  }
  return _pcrVersion;
}

// ---------------------------------------------------------------------------
// Cursor version — detected once at startup, cached for watcher lifetime
// ---------------------------------------------------------------------------

let _cursorVersion: string | null | undefined = undefined; // undefined = not yet checked

function cursorPackageJsonPaths(): string[] {
  const os = platform();
  if (os === "darwin") {
    return [
      "/Applications/Cursor.app/Contents/Resources/app/package.json",
      join(process.env.HOME ?? "", "Applications/Cursor.app/Contents/Resources/app/package.json"),
    ];
  }
  if (os === "win32") {
    const localAppData = process.env.LOCALAPPDATA ?? "";
    const programFiles = process.env.PROGRAMFILES ?? "C:\\Program Files";
    return [
      join(localAppData, "Programs\\Cursor\\resources\\app\\package.json"),
      join(programFiles, "Cursor\\resources\\app\\package.json"),
    ];
  }
  // Linux
  return [
    "/usr/share/cursor/resources/app/package.json",
    "/opt/cursor/resources/app/package.json",
    join(process.env.HOME ?? "", ".local/share/cursor/resources/app/package.json"),
  ];
}

function readCursorVersionFromBundle(): string | null {
  for (const p of cursorPackageJsonPaths()) {
    try {
      if (existsSync(p)) {
        const pkg = JSON.parse(readFileSync(p, "utf-8")) as { version?: string };
        if (pkg.version) return pkg.version;
      }
    } catch {
      // try next path
    }
  }
  return null;
}

function readCursorVersionFromCli(): string | null {
  try {
    const output = execSync("cursor --version 2>/dev/null", {
      encoding: "utf-8",
      timeout: 3000,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
    // Output might be "0.47.1" or "Cursor 0.47.1" etc.
    const match = output.match(/(\d+\.\d+\.\d+)/);
    return match?.[1] ?? null;
  } catch {
    return null;
  }
}

export function getCursorVersion(): string | null {
  if (_cursorVersion !== undefined) return _cursorVersion;
  _cursorVersion = readCursorVersionFromBundle() ?? readCursorVersionFromCli() ?? null;
  if (_cursorVersion) {
    console.error(`PCR: Cursor version: ${_cursorVersion}`);
  } else {
    console.error("PCR: Cursor version could not be detected.");
  }
  return _cursorVersion;
}

/**
 * Returns the provenance fields to include in every prompt's file_context.
 * Call once at watcher startup to log the versions, then reuse the result.
 */
export function getCaptureProvenance(): {
  cursor_version: string | null;
  pcr_version: string;
  capture_schema: number;
} {
  return {
    cursor_version: getCursorVersion(),
    pcr_version: getPcrVersion(),
    capture_schema: CAPTURE_SCHEMA_VERSION,
  };
}
