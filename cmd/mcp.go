package cmd

import (
	"context"
	"fmt"
	"os"

	"github.com/mark3labs/mcp-go/mcp"
	"github.com/mark3labs/mcp-go/server"
	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/supabase"
)

var mcpCmd = &cobra.Command{
	Use:   "mcp",
	Short: "Start the MCP server on stdio",
	RunE: func(cmd *cobra.Command, args []string) error {
		a := auth.Load()
		userID := ""
		token := ""
		if a != nil {
			userID = a.UserID
			token = a.Token
		}

		captureCount := 0

		s := server.NewMCPServer("pcr-dev", Version)

		// ── pcr_log_prompt ────────────────────────────────────────────────────
		s.AddTool(
			mcp.NewTool("pcr_log_prompt",
				mcp.WithDescription("Log a prompt and its AI response for later code review. Call this after completing a coding task."),
				mcp.WithString("prompt_text", mcp.Required(), mcp.Description("The user's prompt or instruction to the AI")),
				mcp.WithString("response_text", mcp.Description("The AI's response (summary or full text)")),
				mcp.WithString("session_id", mcp.Description("Session identifier to group related prompts")),
				mcp.WithString("project_name", mcp.Description("Name of the project being worked on")),
				mcp.WithString("branch_name", mcp.Description("Git branch name")),
				mcp.WithString("model", mcp.Description("AI model used (e.g., claude-sonnet-4-6)")),
				mcp.WithString("source", mcp.Description("AI tool name (e.g., claude-code, cursor, codex)")),
			),
			func(ctx context.Context, req mcp.CallToolRequest) (*mcp.CallToolResult, error) {
				promptText, _ := req.Params.Arguments["prompt_text"].(string)
				responseText, _ := req.Params.Arguments["response_text"].(string)
				sessionID, _ := req.Params.Arguments["session_id"].(string)
				projectName, _ := req.Params.Arguments["project_name"].(string)
				branchName, _ := req.Params.Arguments["branch_name"].(string)
				model, _ := req.Params.Arguments["model"].(string)
				source, _ := req.Params.Arguments["source"].(string)

				if sessionID == "" {
					sessionID = fmt.Sprintf("mcp-%d", captureCount)
				}
				if source == "" {
					source = "unknown"
				}

				record := supabase.PromptRecord{
					SessionID:     sessionID,
					ProjectName:   projectName,
					BranchName:    branchName,
					PromptText:    promptText,
					ResponseText:  responseText,
					Model:         model,
					Source:        source,
					CaptureMethod: "mcp",
					UserID:        userID,
				}

				ok, err := supabase.UpsertPrompt(token, record)
				if ok {
					captureCount++
				}

				msg := fmt.Sprintf("PCR: Prompt logged (session total: %d)", captureCount)
				if err != nil || !ok {
					msg = "PCR: Failed to log prompt — check connection"
				}
				return mcp.NewToolResultText(msg), nil
			},
		)

		// ── pcr_log_session ───────────────────────────────────────────────────
		s.AddTool(
			mcp.NewTool("pcr_log_session",
				mcp.WithDescription("Log an entire coding session transcript. Use at the end of a session to capture all interactions at once."),
				mcp.WithString("session_id", mcp.Required(), mcp.Description("Unique session identifier")),
				mcp.WithString("project_name", mcp.Description("Project name")),
				mcp.WithString("branch_name", mcp.Description("Git branch")),
				mcp.WithString("source", mcp.Description("AI tool name")),
			),
			func(ctx context.Context, req mcp.CallToolRequest) (*mcp.CallToolResult, error) {
				sessionID, _ := req.Params.Arguments["session_id"].(string)
				projectName, _ := req.Params.Arguments["project_name"].(string)
				branchName, _ := req.Params.Arguments["branch_name"].(string)
				source, _ := req.Params.Arguments["source"].(string)
				if source == "" {
					source = "unknown"
				}

				// messages is an array of {role, content, model?, timestamp?}
				messagesRaw, _ := req.Params.Arguments["messages"].([]any)
				var records []supabase.PromptRecord

				for i, msgRaw := range messagesRaw {
					msg, ok := msgRaw.(map[string]any)
					if !ok {
						continue
					}
					role, _ := msg["role"].(string)
					if role != "user" {
						continue
					}
					content, _ := msg["content"].(string)

					var responseText, model string
					if i+1 < len(messagesRaw) {
						if next, ok := messagesRaw[i+1].(map[string]any); ok {
							if nextRole, _ := next["role"].(string); nextRole == "assistant" {
								responseText, _ = next["content"].(string)
								model, _ = next["model"].(string)
							}
						}
					}
					if model == "" {
						model, _ = msg["model"].(string)
					}

					ts, _ := msg["timestamp"].(string)
					records = append(records, supabase.PromptRecord{
						SessionID:     sessionID,
						ProjectName:   projectName,
						BranchName:    branchName,
						PromptText:    content,
						ResponseText:  responseText,
						Model:         model,
						Source:        source,
						CaptureMethod: "mcp",
						CapturedAt:    ts,
						UserID:        userID,
					})
				}

				count, _ := supabase.UpsertPrompts(token, records)
				captureCount += count
				return mcp.NewToolResultText(
					fmt.Sprintf("PCR: Session logged — %d prompt(s) captured (session total: %d)", count, captureCount),
				), nil
			},
		)

		// ── pcr_status ────────────────────────────────────────────────────────
		s.AddTool(
			mcp.NewTool("pcr_status",
				mcp.WithDescription("Check how many prompts have been captured in this PCR session"),
			),
			func(ctx context.Context, req mcp.CallToolRequest) (*mcp.CallToolResult, error) {
				user := userID
				if user == "" {
					user = "not logged in"
				}
				return mcp.NewToolResultText(
					fmt.Sprintf("PCR.dev status: %d prompt(s) captured this session. User: %s", captureCount, user),
				), nil
			},
		)

		fmt.Fprintln(os.Stderr, "PCR: MCP server running on stdio")
		return server.ServeStdio(s)
	},
}
