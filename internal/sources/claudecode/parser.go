package claudecode

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/supabase"
)

// ─── JSONL message types ──────────────────────────────────────────────────────

type messageUsage struct {
	InputTokens              int `json:"input_tokens"`
	OutputTokens             int `json:"output_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
}

type contentBlock struct {
	Type      string           `json:"type"`
	Text      string           `json:"text"`
	Thinking  string           `json:"thinking"`
	ID        string           `json:"id"`
	Name      string           `json:"name"`
	Input     map[string]any   `json:"input"`
	ToolUseID string           `json:"tool_use_id"`
	Content   json.RawMessage  `json:"content"`
	IsError   bool             `json:"is_error"`
}

type messageBody struct {
	Role    string          `json:"role"`
	Content json.RawMessage `json:"content"`
	Model   string          `json:"model"`
	Usage   *messageUsage   `json:"usage"`
}

type claudeCodeMessage struct {
	Type              string      `json:"type"`
	Message           messageBody `json:"message"`
	Timestamp         string      `json:"timestamp"`
	SessionID         string      `json:"session_id"`
	SessionIDCamel    string      `json:"sessionId"`
	IsSidechain        bool        `json:"isSidechain"`
	GitBranch         string      `json:"gitBranch"`
}

// ParsedSession is the result of parsing a Claude Code JSONL file.
type ParsedSession struct {
	SessionID         string
	ProjectName       string
	Branch            string
	Model             string
	TotalInputTokens  int
	TotalOutputTokens int
	ExchangeCount     int
	SessionCreatedAt  string
	SessionUpdatedAt  string
	Prompts           []supabase.PromptRecord
}

// ParseClaudeCodeSession parses a full JSONL file into structured prompt records.
func ParseClaudeCodeSession(fileContent, projectName, filePath string) ParsedSession {
	lines := strings.Split(strings.TrimSpace(fileContent), "\n")
	var messages []claudeCodeMessage

	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var msg claudeCodeMessage
		if err := json.Unmarshal([]byte(line), &msg); err != nil {
			continue
		}
		if msg.IsSidechain {
			continue
		}
		if msg.Type != "" {
			messages = append(messages, msg)
		}
	}

	// Extract branch from system or first human message
	branch := ""
	for _, m := range messages {
		if m.Type == "system" && m.GitBranch != "" {
			branch = m.GitBranch
			break
		}
	}
	if branch == "" {
		for _, m := range messages {
			if (m.Type == "human" || m.Type == "user") && m.GitBranch != "" {
				branch = m.GitBranch
				break
			}
		}
	}

	// Derive session ID
	sessionID := ""
	if len(messages) > 0 {
		sessionID = messages[0].SessionID
		if sessionID == "" {
			sessionID = messages[0].SessionIDCamel
		}
	}
	if sessionID == "" {
		base := filepath.Base(filePath)
		sessionID = strings.TrimSuffix(base, ".jsonl")
	}
	if sessionID == "" {
		sessionID = "session-" + time.Now().Format("20060102150405")
	}

	var (
		prompts           []supabase.PromptRecord
		totalInput        int
		totalOutput       int
		sessionModel      string
		sessionCreatedAt  string
		sessionUpdatedAt  string
	)

	for i, msg := range messages {
		if msg.Timestamp != "" {
			if sessionCreatedAt == "" {
				sessionCreatedAt = msg.Timestamp
			}
			sessionUpdatedAt = msg.Timestamp
		}
		if msg.Type != "human" && msg.Type != "user" {
			continue
		}
		promptText := extractText(msg.Message.Content)
		if strings.TrimSpace(promptText) == "" {
			continue
		}

		var (
			responseText   string
			model          string
			toolCalls      []map[string]any
			toolResults    []map[string]any
			thinkingContent string
			inputTokens    int
			outputTokens   int
		)

		for j := i + 1; j < len(messages); j++ {
			next := messages[j]
			if next.Type == "assistant" {
				chunk := extractText(next.Message.Content)
				if chunk != "" {
					if responseText != "" {
						responseText += "\n" + chunk
					} else {
						responseText = chunk
					}
				}
				if t := extractThinking(next.Message.Content); t != "" {
					if thinkingContent != "" {
						thinkingContent += "\n" + t
					} else {
						thinkingContent = t
					}
				}
				if model == "" && next.Message.Model != "" {
					model = next.Message.Model
					if sessionModel == "" {
						sessionModel = model
					}
				}
				tools := extractToolCalls(next.Message.Content)
				toolCalls = append(toolCalls, tools...)

				if next.Message.Usage != nil {
					inputTokens += next.Message.Usage.InputTokens
					outputTokens += next.Message.Usage.OutputTokens
					totalInput += next.Message.Usage.InputTokens
					totalOutput += next.Message.Usage.OutputTokens
				}
				continue
			}
			if next.Type == "human" || next.Type == "user" {
				results := extractToolResults(next.Message.Content)
				toolResults = append(toolResults, results...)
				if strings.TrimSpace(extractText(next.Message.Content)) != "" {
					break
				}
				continue
			}
			// Skip auxiliary message types (attachment, file-history-snapshot, etc.)
			// that don't represent conversation boundaries.
			if next.Type == "system" {
				break
			}
		}

		fileContext := map[string]any{}
		if len(toolResults) > 0 {
			fileContext["tool_results"] = toolResults
		}
		if thinkingContent != "" {
			fileContext["thinking_content"] = thinkingContent
		}
		if inputTokens > 0 {
			fileContext["input_tokens"] = inputTokens
		}
		if outputTokens > 0 {
			fileContext["output_tokens"] = outputTokens
		}

		capturedAt := msg.Timestamp
		if capturedAt == "" {
			capturedAt = time.Now().UTC().Format(time.RFC3339)
		}

		prompts = append(prompts, supabase.PromptRecord{
			SessionID:     sessionID,
			ProjectName:   projectName,
			BranchName:    branch,
			PromptText:    promptText,
			ResponseText:  responseText,
			Model:         model,
			Source:        "claude-code",
			CaptureMethod: "file-watcher",
			ToolCalls:     toolCalls,
			FileContext:   fileContext,
			CapturedAt:    capturedAt,
		})
	}

	return ParsedSession{
		SessionID:         sessionID,
		ProjectName:       projectName,
		Branch:            branch,
		Model:             sessionModel,
		TotalInputTokens:  totalInput,
		TotalOutputTokens: totalOutput,
		ExchangeCount:     len(prompts),
		SessionCreatedAt:  sessionCreatedAt,
		SessionUpdatedAt:  sessionUpdatedAt,
		Prompts:           prompts,
	}
}

// ─── Content extraction helpers ───────────────────────────────────────────────

func parseContent(raw json.RawMessage) []contentBlock {
	if len(raw) == 0 {
		return nil
	}
	// Try array first
	var blocks []contentBlock
	if err := json.Unmarshal(raw, &blocks); err == nil {
		return blocks
	}
	// Fall back to plain string
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return []contentBlock{{Type: "text", Text: s}}
	}
	return nil
}

func extractText(raw json.RawMessage) string {
	blocks := parseContent(raw)
	var parts []string
	for _, b := range blocks {
		if b.Type == "text" && b.Text != "" {
			parts = append(parts, b.Text)
		}
	}
	return strings.Join(parts, "\n")
}

func extractThinking(raw json.RawMessage) string {
	blocks := parseContent(raw)
	var parts []string
	for _, b := range blocks {
		if b.Type == "thinking" && b.Thinking != "" {
			parts = append(parts, b.Thinking)
		}
	}
	return strings.Join(parts, "\n")
}

func extractToolCalls(raw json.RawMessage) []map[string]any {
	blocks := parseContent(raw)
	var result []map[string]any
	for _, b := range blocks {
		if b.Type == "tool_use" {
			result = append(result, map[string]any{
				"tool":  b.Name,
				"input": b.Input,
				"id":    b.ID,
			})
		}
	}
	return result
}

func extractToolResults(raw json.RawMessage) []map[string]any {
	blocks := parseContent(raw)
	var result []map[string]any
	for _, b := range blocks {
		if b.Type != "tool_result" {
			continue
		}
		var raw string
		if b.Content != nil {
			var s string
			if err := json.Unmarshal(b.Content, &s); err == nil {
				raw = s
			} else {
				var subBlocks []contentBlock
				if err := json.Unmarshal(b.Content, &subBlocks); err == nil {
					var parts []string
					for _, sb := range subBlocks {
						if sb.Type == "text" && sb.Text != "" {
							parts = append(parts, sb.Text)
						}
					}
					raw = strings.Join(parts, "\n")
				}
			}
		}
		if len(raw) > 500 {
			raw = raw[:497] + "…"
		}
		result = append(result, map[string]any{
			"tool_use_id": b.ToolUseID,
			"content":     raw,
			"is_error":    b.IsError,
		})
	}
	return result
}
