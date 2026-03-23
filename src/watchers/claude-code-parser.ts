/**
 * Claude Code stores sessions as JSONL files at:
 *   ~/.claude/projects/<project-slug>/<session-id>.jsonl
 *
 * The project slug is the workspace path with slashes replaced by hyphens.
 * e.g. /Users/kalujo/Desktop/PCR.dev -> Users-kalujo-Desktop-PCR.dev
 */

import { PromptRecord } from "../lib/supabase.js";

interface ClaudeCodeMessage {
  type: "human" | "assistant" | "system";
  message: {
    role: string;
    content: string | ContentBlock[];
    model?: string;
  };
  timestamp?: string;
  session_id?: string;
}

interface ContentBlock {
  type: "text" | "tool_use" | "tool_result";
  text?: string;
  name?: string;
  input?: Record<string, unknown>;
  content?: string | ContentBlock[];
}

export interface ParsedClaudeSession {
  session_id: string;
  project_name: string;
  prompts: PromptRecord[];
}

function extractText(content: string | ContentBlock[]): string {
  if (typeof content === "string") return content;
  return content
    .filter((block) => block.type === "text" && block.text)
    .map((block) => block.text!)
    .join("\n");
}

function extractToolCalls(content: string | ContentBlock[]): Record<string, unknown>[] {
  if (typeof content === "string") return [];
  return content
    .filter((block) => block.type === "tool_use")
    .map((block) => ({ tool: block.name, input: block.input }));
}

export function parseClaudeCodeSession(
  fileContent: string,
  projectName: string,
  filePath: string
): ParsedClaudeSession {
  const lines = fileContent.trim().split("\n");
  const messages: ClaudeCodeMessage[] = [];

  for (const line of lines) {
    if (!line.trim()) continue;
    try {
      const parsed = JSON.parse(line) as ClaudeCodeMessage;
      if (parsed.type && parsed.message) messages.push(parsed);
    } catch {
      // Skip malformed lines — file may still be written to
    }
  }

  const sessionId =
    messages[0]?.session_id ||
    filePath.split("/").pop()?.replace(".jsonl", "") ||
    `session-${Date.now()}`;

  const prompts: PromptRecord[] = [];

  for (let i = 0; i < messages.length; i++) {
    const msg = messages[i];
    if (msg.type !== "human") continue;

    const promptText = extractText(msg.message.content);
    if (!promptText.trim()) continue;

    let responseText: string | undefined;
    let model: string | undefined;
    let toolCalls: Record<string, unknown>[] | undefined;

    if (i + 1 < messages.length && messages[i + 1].type === "assistant") {
      const next = messages[i + 1];
      responseText = extractText(next.message.content);
      model = next.message.model;
      const tools = extractToolCalls(next.message.content);
      if (tools.length > 0) toolCalls = tools;
    }

    prompts.push({
      session_id: sessionId,
      project_name: projectName,
      prompt_text: promptText,
      response_text: responseText,
      model,
      source: "claude-code",
      capture_method: "file-watcher",
      tool_calls: toolCalls,
      captured_at: msg.timestamp || new Date().toISOString(),
    });
  }

  return { session_id: sessionId, project_name: projectName, prompts };
}
