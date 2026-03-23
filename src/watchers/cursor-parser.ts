/**
 * Parses Cursor agent-transcript JSONL files from:
 *   ~/.cursor/projects/<project-slug>/agent-transcripts/<uuid>/<uuid>.jsonl
 *
 * The project slug is the workspace path with slashes and dots replaced by
 * hyphens. e.g. /Users/kalujo/Desktop/PCR.dev -> Users-kalujo-Desktop-PCR-dev
 */

import { PromptRecord } from "../lib/supabase.js";

interface CursorMessage {
  role: "user" | "assistant";
  message: {
    content: ContentBlock[] | string;
  };
}

interface ContentBlock {
  type: "text" | string;
  text?: string;
}

export interface ParsedCursorSession {
  session_id: string;
  project_name: string;
  prompts: PromptRecord[];
}

function extractText(content: ContentBlock[] | string): string {
  if (typeof content === "string") return content;
  return content
    .filter((b) => b.type === "text" && b.text)
    .map((b) => b.text!)
    .join("\n");
}

/**
 * Convert project slug to a readable project name.
 * "Users-kalujo-Desktop-my-app" -> "my-app" (last segment after known prefixes)
 */
function slugToProjectName(slug: string): string {
  const parts = slug.split("-");
  const knownPrefixes = ["Users", "home", "Desktop", "Documents", "Projects", "code", "dev"];
  let i = 0;
  while (i < parts.length && knownPrefixes.includes(parts[i])) i++;
  i++; // skip username
  return parts.slice(i).join("-") || slug;
}

export function parseCursorTranscript(
  fileContent: string,
  sessionUuid: string,
  projectSlug: string
): ParsedCursorSession {
  const projectName = slugToProjectName(projectSlug);
  const lines = fileContent.trim().split("\n");
  const messages: CursorMessage[] = [];

  for (const line of lines) {
    if (!line.trim()) continue;
    try {
      const parsed = JSON.parse(line) as CursorMessage;
      if (parsed.role && parsed.message) messages.push(parsed);
    } catch {
      // Skip malformed lines
    }
  }

  const prompts: PromptRecord[] = [];

  for (let i = 0; i < messages.length; i++) {
    const msg = messages[i];
    if (msg.role !== "user") continue;

    const promptText = extractText(msg.message.content);
    if (!promptText.trim()) continue;

    let responseText: string | undefined;
    if (i + 1 < messages.length && messages[i + 1].role === "assistant") {
      responseText = extractText(messages[i + 1].message.content);
    }

    prompts.push({
      session_id: sessionUuid,
      project_name: projectName,
      prompt_text: promptText,
      response_text: responseText,
      source: "cursor",
      capture_method: "file-watcher",
      captured_at: new Date().toISOString(),
    });
  }

  return { session_id: sessionUuid, project_name: projectName, prompts };
}
