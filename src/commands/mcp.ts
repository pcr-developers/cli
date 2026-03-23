import { createRequire } from "module";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { insertPrompt, insertPromptsBatch, PromptRecord } from "../lib/supabase.js";
import { loadAuth } from "../lib/auth.js";

const require = createRequire(import.meta.url);
const pkg = require("../../package.json") as { version: string };

let captureCount = 0;

export async function runMcp(): Promise<void> {
  const auth = loadAuth();
  const userId = auth?.userId;

  const server = new McpServer({
    name: "pcr-dev",
    version: pkg.version,
  });

  server.registerTool(
    "pcr_log_prompt",
    {
      title: "Log Prompt",
      description:
        "Log a prompt and its AI response for later code review. Call this after completing a coding task.",
      inputSchema: {
        prompt_text: z.string().describe("The user's prompt or instruction to the AI"),
        response_text: z.string().optional().describe("The AI's response (summary or full text)"),
        session_id: z.string().optional().describe("Session identifier to group related prompts"),
        project_name: z.string().optional().describe("Name of the project being worked on"),
        branch_name: z.string().optional().describe("Git branch name"),
        model: z.string().optional().describe("AI model used (e.g., claude-sonnet-4-5)"),
        source: z.string().optional().describe("AI tool name (e.g., claude-code, cursor, codex)"),
        files_changed: z.array(z.string()).optional().describe("List of files modified"),
      },
    },
    async (params) => {
      const record: PromptRecord = {
        session_id: params.session_id || `mcp-${Date.now()}`,
        project_name: params.project_name,
        branch_name: params.branch_name,
        prompt_text: params.prompt_text,
        response_text: params.response_text,
        model: params.model,
        source: params.source || "unknown",
        capture_method: "mcp",
        file_context: params.files_changed ? { files: params.files_changed } : undefined,
        captured_at: new Date().toISOString(),
        user_id: userId,
      };

      const success = await insertPrompt(record);
      if (success) captureCount++;

      return {
        content: [
          {
            type: "text" as const,
            text: success
              ? `PCR: Prompt logged (session total: ${captureCount})`
              : "PCR: Failed to log prompt — check connection",
          },
        ],
      };
    }
  );

  server.registerTool(
    "pcr_log_session",
    {
      title: "Log Session",
      description:
        "Log an entire coding session transcript. Use at the end of a session to capture all interactions at once.",
      inputSchema: {
        session_id: z.string().describe("Unique session identifier"),
        project_name: z.string().optional().describe("Project name"),
        branch_name: z.string().optional().describe("Git branch"),
        source: z.string().optional().describe("AI tool name"),
        messages: z
          .array(
            z.object({
              role: z.enum(["user", "assistant"]),
              content: z.string(),
              model: z.string().optional(),
              timestamp: z.string().optional(),
            })
          )
          .describe("Array of messages in the session"),
      },
    },
    async (params) => {
      const records: PromptRecord[] = [];

      for (let i = 0; i < params.messages.length; i++) {
        const msg = params.messages[i];
        if (msg.role !== "user") continue;

        let responseText: string | undefined;
        let model: string | undefined;
        if (i + 1 < params.messages.length && params.messages[i + 1].role === "assistant") {
          responseText = params.messages[i + 1].content;
          model = params.messages[i + 1].model;
        }

        records.push({
          session_id: params.session_id,
          project_name: params.project_name,
          branch_name: params.branch_name,
          prompt_text: msg.content,
          response_text: responseText,
          model: model || msg.model,
          source: params.source || "unknown",
          capture_method: "mcp",
          captured_at: msg.timestamp || new Date().toISOString(),
          user_id: userId,
        });
      }

      const count = await insertPromptsBatch(records);
      captureCount += count;

      return {
        content: [
          {
            type: "text" as const,
            text: `PCR: Session logged — ${count} prompt(s) captured (session total: ${captureCount})`,
          },
        ],
      };
    }
  );

  server.registerTool(
    "pcr_status",
    {
      title: "PCR Status",
      description: "Check how many prompts have been captured in this PCR session",
    },
    async () => {
      return {
        content: [
          {
            type: "text" as const,
            text: `PCR.dev status: ${captureCount} prompt(s) captured this session. User: ${userId ?? "not logged in"}`,
          },
        ],
      };
    }
  );

  const transport = new StdioServerTransport();
  await server.connect(transport);
  // MCP server owns stdout — all other output must go to stderr
  console.error("PCR: MCP server running on stdio");
}
