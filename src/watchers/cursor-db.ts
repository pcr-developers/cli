/**
 * Reads enriched conversation metadata from Cursor's SQLite database.
 *
 * Cursor stores rich session data (timing, agentic status, relevant files)
 * in ~/Library/Application Support/Cursor/User/globalStorage/state.vscdb
 * that is NOT written to the JSONL transcript files we watch.
 *
 * This module reads that database to enrich captures with:
 *   - isAgentic: per-bubble — whether agent capabilities ran on that turn
 *   - submittedAt: per-bubble — epoch ms when the user actually submitted
 *   - responseDurationMs: per-bubble — how long the AI took to respond
 *   - relevantFiles: per-bubble — files Cursor pulled into context
 *
 * Note: session-level `unifiedMode` ("agent"/"chat"/"plan") is intentionally
 * NOT captured. It reflects the current mode of the conversation, not the mode
 * at the time of each message, so it stamps all historical prompts with the
 * most recently used mode. Per-bubble `isAgentic` is the only reliable signal.
 *
 * Database is opened read-only so we never block Cursor's writes.
 * Results are cached per sessionId with a 5-minute TTL.
 */

import { createRequire } from "module";
import { existsSync } from "fs";
import { join } from "path";
import { homedir, platform } from "os";

const require = createRequire(import.meta.url);

// ---------------------------------------------------------------------------
// DB path (cross-platform)
// ---------------------------------------------------------------------------

function getCursorDbPath(): string {
  const os = platform();
  if (os === "darwin") {
    return join(homedir(), "Library/Application Support/Cursor/User/globalStorage/state.vscdb");
  }
  if (os === "win32") {
    const appData = process.env.APPDATA ?? join(homedir(), "AppData/Roaming");
    return join(appData, "Cursor/User/globalStorage/state.vscdb");
  }
  // Linux
  return join(homedir(), ".config/Cursor/User/globalStorage/state.vscdb");
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface BubbleMeta {
  /** 1 = user turn, 2 = assistant turn */
  type: 1 | 2;
  /**
   * Whether this specific turn used agent capabilities.
   * Read from per-bubble data — accurate per-turn, unlike session-level mode.
   * Only present on assistant (type 2) bubbles.
   */
  isAgentic?: boolean;
  /**
   * When the user submitted this turn (epoch ms).
   * Sourced from assistantBubble.timingInfo.clientStartTime — the moment the
   * AI started processing, which is immediately after the user submitted.
   * Only present on assistant (type 2) bubbles where timingInfo is available.
   */
  submittedAt?: number;
  /** How long the model took to respond (ms). Assistant turns only. */
  responseDurationMs?: number;
  /** Files Cursor included in context for this turn. */
  relevantFiles?: string[];
  /**
   * Per-turn code change data (old format _v < 14 only).
   * diffHistories: full file diffs applied during this turn.
   * humanChanges: edits made by the human in this turn.
   * fileDiffTrajectories: trajectory of file changes across tools.
   */
  diffHistories?: unknown[];
  humanChanges?: unknown[];
  fileDiffTrajectories?: unknown[];
}

export interface CursorSessionMeta {
  /** Per-turn metadata, ordered to match JSONL turns */
  bubbles: BubbleMeta[];
  /** Model used for this session, e.g. "claude-sonnet-4-5" (available in _v >= 14) */
  modelName?: string;
  /** Whether the session used agent mode */
  isAgentic?: boolean;
  /**
   * Session-level mode: "agent" | "chat" | "plan" | "normal".
   * Unlike isAgentic this is reliable for dedicated Plan/Debug sessions where
   * the user never switches modes mid-conversation.
   */
  unifiedMode?: string;
}

/** Full composerData snapshot for the cursor_sessions table. */
export interface CursorSessionData {
  sessionId: string;
  schemaV: number;
  name?: string;
  subtitle?: string;
  modelName?: string;
  isAgentic?: boolean;
  unifiedMode?: string;
  planModeUsed?: boolean;
  debugModeUsed?: boolean;
  branch?: string;
  contextTokensUsed?: number;
  contextTokenLimit?: number;
  filesChangedCount?: number;
  totalLinesAdded?: number;
  totalLinesRemoved?: number;
  sessionCreatedAt?: number;   // epoch ms from Cursor
  sessionUpdatedAt?: number;
  /** Git commits made while this session was open (populated by cursor.ts via git log). */
  commitShaStart?: string;
  commitShaEnd?: string;
  commitShas?: string[];
  /** Everything else from composerData that doesn't have a dedicated column. */
  meta: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Lazy DB handle
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type SQLiteDb = any;

let _db: SQLiteDb | null = null;
let _dbUnavailable = false;

function openDb(): SQLiteDb | null {
  if (_dbUnavailable) return null;
  if (_db) return _db;

  const dbPath = getCursorDbPath();
  if (!existsSync(dbPath)) {
    _dbUnavailable = true;
    return null;
  }

  try {
    const Database = require("better-sqlite3");
    _db = new Database(dbPath, { readonly: true, fileMustExist: true });
    return _db;
  } catch {
    _dbUnavailable = true;
    return null;
  }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

const cache = new Map<string, CursorSessionMeta | null>();
const cacheTimestamps = new Map<string, number>();
const CACHE_TTL_MS = 5 * 60 * 1000;

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/**
 * Look up enriched metadata for a Cursor session.
 *
 * Returns null if:
 * - better-sqlite3 is not installed
 * - Cursor's DB doesn't exist
 * - The sessionId has no matching composerData
 */
export function getSessionMeta(sessionId: string): CursorSessionMeta | null {
  // Cache hit
  const cachedAt = cacheTimestamps.get(sessionId);
  if (cachedAt !== undefined && Date.now() - cachedAt < CACHE_TTL_MS) {
    return cache.get(sessionId) ?? null;
  }

  const db = openDb();
  if (!db) return storeAndReturn(sessionId, null);

  try {
    // Read only the fields we need — avoids loading 100MB+ blobs into JS.
    const row = db
      .prepare(
        `SELECT
           json_extract(value, '$._v')                           as schema_v,
           json_extract(value, '$.isAgentic')                   as is_agentic,
           json_extract(value, '$.unifiedMode')                 as unified_mode,
           json_extract(value, '$.modelConfig')                 as model_config,
           json_extract(value, '$.conversation')                as conversation,
           json_extract(value, '$.fullConversationHeadersOnly') as headers_only
         FROM cursorDiskKV
         WHERE key = ?`
      )
      .get(`composerData:${sessionId}`) as {
        schema_v: number | null;
        is_agentic: number | null;
        unified_mode: string | null;
        model_config: string | null;
        conversation: string | null;
        headers_only: string | null;
      } | undefined;

    if (!row) return storeAndReturn(sessionId, null);

    const schemaV = row.schema_v ?? 0;
    const isAgentic = row.is_agentic === 1;
    const unifiedMode = row.unified_mode ?? undefined;

    // Extract model name (available in _v >= 14)
    let modelName: string | undefined;
    if (row.model_config) {
      try {
        const mc = JSON.parse(row.model_config) as { modelName?: string };
        if (mc.modelName) modelName = mc.modelName;
      } catch { /* ignore */ }
    }

    const bubbles: BubbleMeta[] = [];

    if (schemaV >= 14) {
      // New format (_v 14+): conversation array replaced by fullConversationHeadersOnly
      // (just {bubbleId, type} — no timing/isAgentic per bubble) and conversationMap (empty).
      // We can still build a minimal bubble list for ordering purposes.
      if (row.headers_only) {
        try {
          const headers = JSON.parse(row.headers_only) as Array<{ type: 1 | 2 }>;
          for (const h of headers) {
            bubbles.push({ type: h.type });
          }
        } catch { /* ignore */ }
      }
    } else {
      // Legacy format: full conversation array with per-bubble timing and isAgentic
      if (row.conversation) {
        try {
          const conv = JSON.parse(row.conversation) as Array<{
            type: 1 | 2;
            isAgentic?: boolean;
            timingInfo?: {
              clientStartTime?: number;
              clientRpcSendTime?: number;
              clientSettleTime?: number;
            };
            relevantFiles?: string[];
            diffHistories?: unknown[];
            humanChanges?: unknown[];
            fileDiffTrajectories?: unknown[];
          }>;

          for (const bubble of conv) {
            const b: BubbleMeta = { type: bubble.type };

            if (bubble.type === 2) {
              if (bubble.isAgentic !== undefined) b.isAgentic = Boolean(bubble.isAgentic);

              if (bubble.timingInfo?.clientStartTime) {
                b.submittedAt = bubble.timingInfo.clientStartTime;
              }
              if (bubble.timingInfo) {
                const { clientRpcSendTime, clientSettleTime } = bubble.timingInfo;
                if (clientRpcSendTime && clientSettleTime && clientSettleTime > clientRpcSendTime) {
                  b.responseDurationMs = clientSettleTime - clientRpcSendTime;
                }
              }

              // Per-turn code change data (old format only — moved to blobs in _v >= 14)
              if (bubble.diffHistories?.length)        b.diffHistories        = bubble.diffHistories;
              if (bubble.humanChanges?.length)         b.humanChanges         = bubble.humanChanges;
              if (bubble.fileDiffTrajectories?.length) b.fileDiffTrajectories = bubble.fileDiffTrajectories;
            }

            if (bubble.relevantFiles?.length) b.relevantFiles = bubble.relevantFiles;
            bubbles.push(b);
          }
        } catch {
          return storeAndReturn(sessionId, null);
        }
      }
    }

    return storeAndReturn(sessionId, { bubbles, modelName, isAgentic, unifiedMode });
  } catch {
    return storeAndReturn(sessionId, null);
  }
}

function storeAndReturn(
  sessionId: string,
  value: CursorSessionMeta | null
): CursorSessionMeta | null {
  cache.set(sessionId, value);
  cacheTimestamps.set(sessionId, Date.now());
  return value;
}

/**
 * Format response duration for display: "1.2s", "450ms"
 */
export function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${ms}ms`;
}

// ---------------------------------------------------------------------------
// Full session snapshot for cursor_sessions table
// ---------------------------------------------------------------------------

// Fields too large or not useful to store
const STRIPPED_FIELDS = new Set([
  "fullConversationHeadersOnly",
  "conversationMap",
  "conversationState",
  "blobEncryptionKey",
  "speculativeSummarizationEncryptionKey",
  "richText",
  "generatingBubbleIds",
  "codeBlockData",
  "originalFileStates",  // large internal state
]);

/**
 * Read the full composerData snapshot for a session and return it as a
 * structured object suitable for the cursor_sessions table.
 *
 * Separate from getSessionMeta so the per-bubble cache is not polluted with
 * the large full-snapshot read.
 */
export function getFullSessionData(sessionId: string): CursorSessionData | null {
  const db = openDb();
  if (!db) return null;

  try {
    const row = db
      .prepare("SELECT value FROM cursorDiskKV WHERE key = ?")
      .get(`composerData:${sessionId}`) as { value: string } | undefined;

    if (!row) return null;

    const obj = JSON.parse(row.value) as Record<string, unknown>;

    // Structured columns
    const schemaV = (obj._v as number) ?? 0;
    const isAgentic = obj.isAgentic === true || obj.isAgentic === 1;
    const unifiedMode = (obj.unifiedMode as string) ?? undefined;

    let modelName: string | undefined;
    if (obj.modelConfig && typeof obj.modelConfig === "object") {
      modelName = (obj.modelConfig as { modelName?: string }).modelName;
    }

    const activeBranch = obj.activeBranch as { branchName?: string } | undefined;
    const branch = activeBranch?.branchName;

    // Build meta: everything not in structured columns and not stripped
    const STRUCTURED = new Set([
      "_v", "composerId", "isAgentic", "unifiedMode", "forceMode",
      "modelConfig", "name", "subtitle", "planModeSuggestionUsed",
      "debugModeSuggestionUsed", "contextTokensUsed", "contextTokenLimit",
      "filesChangedCount", "totalLinesAdded", "totalLinesRemoved",
      "activeBranch", "createdOnBranch", "createdAt", "lastUpdatedAt",
    ]);

    const meta: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(obj)) {
      if (!STRUCTURED.has(k) && !STRIPPED_FIELDS.has(k) && v !== null && v !== undefined) {
        meta[k] = v;
      }
    }

    return {
      sessionId,
      schemaV,
      name: (obj.name as string) || undefined,
      subtitle: (obj.subtitle as string) || undefined,
      modelName,
      isAgentic,
      unifiedMode,
      planModeUsed: (obj.planModeSuggestionUsed as boolean) ?? undefined,
      debugModeUsed: (obj.debugModeSuggestionUsed as boolean) ?? undefined,
      branch,
      contextTokensUsed: (obj.contextTokensUsed as number) ?? undefined,
      contextTokenLimit: (obj.contextTokenLimit as number) ?? undefined,
      filesChangedCount: (obj.filesChangedCount as number) ?? undefined,
      totalLinesAdded: (obj.totalLinesAdded as number) ?? undefined,
      totalLinesRemoved: (obj.totalLinesRemoved as number) ?? undefined,
      sessionCreatedAt: (obj.createdAt as number) ?? undefined,
      sessionUpdatedAt: (obj.lastUpdatedAt as number) ?? undefined,
      meta,
    };
  } catch {
    return null;
  }
}
