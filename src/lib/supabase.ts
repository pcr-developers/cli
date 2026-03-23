import { createHash } from "crypto";
import { createClient, SupabaseClient } from "@supabase/supabase-js";
import { PCR_SUPABASE_URL, PCR_SUPABASE_KEY } from "./constants.js";

let client: SupabaseClient | null = null;

export function getSupabase(): SupabaseClient {
  if (!client) {
    client = createClient(PCR_SUPABASE_URL, PCR_SUPABASE_KEY);
  }
  return client;
}

export interface PromptRecord {
  id?: string;
  content_hash?: string;
  session_id: string;
  project_id?: string;
  project_name?: string;
  branch_name?: string;
  prompt_text: string;
  response_text?: string;
  model?: string;
  source: string;
  capture_method: string;
  tool_calls?: Record<string, unknown>[];
  file_context?: Record<string, unknown>;
  captured_at?: string;
  user_id?: string;
  team_id?: string;
}

/**
 * Generate a deterministic UUID from session_id + prompt_text + response_text.
 * Used as the row `id`. Same hash as promptContentHash but formatted as UUID.
 */
export function promptId(sessionId: string, promptText: string, responseText?: string): string {
  const hash = createHash("sha256")
    .update(`${sessionId}\x00${promptText}\x00${responseText ?? ""}`)
    .digest("hex");
  return [
    hash.slice(0, 8),
    hash.slice(8, 12),
    hash.slice(12, 16),
    hash.slice(16, 20),
    hash.slice(20, 32),
  ].join("-");
}

/**
 * Raw SHA-256 hex of session_id + prompt_text + response_text.
 * Stored in the content_hash column. Used as the ON CONFLICT target for upsert
 * so that even rows with old random UUIDs are matched correctly.
 */
export function promptContentHash(sessionId: string, promptText: string, responseText?: string): string {
  return createHash("sha256")
    .update(`${sessionId}\x00${promptText}\x00${responseText ?? ""}`)
    .digest("hex");
}

/**
 * Upsert a single prompt via the upsert_prompts RPC.
 */
export async function insertPrompt(record: PromptRecord): Promise<boolean> {
  const count = await insertPromptsBatch([record]);
  return count > 0;
}

/**
 * Upsert a batch of prompts via the upsert_prompts SECURITY DEFINER RPC.
 *
 * The `id` (primary key) is intentionally omitted — the DB generates a random
 * UUID via gen_random_uuid(). Dedup is handled solely by `content_hash` which
 * has a unique constraint. This avoids PK collisions that would occur if the
 * same content is re-computed with a deterministic id that already exists but
 * under a row with a different content_hash.
 *
 * The RPC's ON CONFLICT (content_hash) DO UPDATE merges enrichment fields:
 *   - captured_at  → keep the earlier timestamp (composerData is more accurate)
 *   - file_context → always refresh (cursor_version, is_agentic may be new)
 *   - model        → never overwrite a known model with null (user may have tagged)
 *   - user_id, project_id → coalesce (once set, keep)
 *   - response_text → coalesce (keep existing if new capture was partial)
 */
// ---------------------------------------------------------------------------
// Cursor session upsert
// ---------------------------------------------------------------------------

import type { CursorSessionData } from "../watchers/cursor-db.js";

/**
 * Upsert a cursor_sessions row via the SECURITY DEFINER RPC.
 * Called once per JSONL file processed (not once per prompt).
 * Errors are swallowed — session metadata is enrichment, not critical.
 */
export async function upsertCursorSession(
  data: CursorSessionData,
  projectId: string | undefined,
  userId: string | undefined
): Promise<void> {
  const payload = {
    session_id:           data.sessionId,
    project_id:           projectId ?? null,
    user_id:              userId ?? null,
    name:                 data.name ?? null,
    subtitle:             data.subtitle ?? null,
    model_name:           data.modelName ?? null,
    is_agentic:           data.isAgentic ?? null,
    unified_mode:         data.unifiedMode ?? null,
    plan_mode_used:       data.planModeUsed ?? null,
    debug_mode_used:      data.debugModeUsed ?? null,
    branch:               data.branch ?? null,
    cursor_schema_v:      data.schemaV,
    context_tokens_used:  data.contextTokensUsed ?? null,
    context_token_limit:  data.contextTokenLimit ?? null,
    files_changed_count:  data.filesChangedCount ?? null,
    total_lines_added:    data.totalLinesAdded ?? null,
    total_lines_removed:  data.totalLinesRemoved ?? null,
    session_created_at:   data.sessionCreatedAt
      ? new Date(data.sessionCreatedAt).toISOString() : null,
    session_updated_at:   data.sessionUpdatedAt
      ? new Date(data.sessionUpdatedAt).toISOString() : null,
    meta: data.meta,
    commit_sha_start: data.commitShaStart ?? null,
    commit_sha_end:   data.commitShaEnd   ?? null,
    commit_shas:      data.commitShas?.length ? data.commitShas : null,
  };

  const { error } = await getSupabase().rpc("upsert_cursor_session", { p_session: payload });
  if (error) {
    console.error("PCR: Failed to upsert session:", error.message);
  }
}

export async function insertPromptsBatch(records: PromptRecord[]): Promise<number> {
  if (records.length === 0) return 0;

  // Only compute content_hash — do NOT set id. The DB generates a fresh
  // gen_random_uuid() on INSERT and ON CONFLICT (content_hash) handles dedup.
  const enriched = records.map((r) => ({
    ...r,
    id: undefined,  // explicitly omit so the DB generates it
    content_hash: r.content_hash ?? promptContentHash(r.session_id, r.prompt_text, r.response_text),
  }));

  const { data, error } = await getSupabase().rpc("upsert_prompts", {
    p_records: enriched,
  });

  if (error) {
    console.error("PCR: Batch upsert failed:", error.message);
    return 0;
  }

  return (data as number) ?? 0;
}
