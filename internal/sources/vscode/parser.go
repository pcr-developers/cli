package vscode

import (
	"encoding/json"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/supabase"
	"github.com/pcr-developers/cli/internal/versions"
)

// ─── JSONL event types ────────────────────────────────────────────────────────

type transcriptEvent struct {
	Type      string          `json:"type"`
	Data      json.RawMessage `json:"data"`
	ID        string          `json:"id"`
	Timestamp string          `json:"timestamp"`
	ParentID  *string         `json:"parentId"`
}

type sessionStartData struct {
	SessionID      string `json:"sessionId"`
	CopilotVersion string `json:"copilotVersion"`
	VSCodeVersion  string `json:"vscodeVersion"`
	StartTime      string `json:"startTime"`
}

type toolRequest struct {
	ToolCallID string `json:"toolCallId"`
	Name       string `json:"name"`
	Arguments  string `json:"arguments"`
	Type       string `json:"type"`
}

type assistantMessageData struct {
	MessageID    string        `json:"messageId"`
	Content      string        `json:"content"`
	ToolRequests []toolRequest `json:"toolRequests"`
	ReasoningText string      `json:"reasoningText"`
}

type userMessageData struct {
	Content     string          `json:"content"`
	Attachments json.RawMessage `json:"attachments"`
}

type turnData struct {
	TurnID string `json:"turnId"`
}

type toolExecStartData struct {
	ToolCallID string          `json:"toolCallId"`
	ToolName   string          `json:"toolName"`
	Arguments  json.RawMessage `json:"arguments"`
}

type toolExecCompleteData struct {
	ToolCallID string `json:"toolCallId"`
	Success    bool   `json:"success"`
}

// ParsedExchange represents a single user→assistant exchange.
type ParsedExchange struct {
	PromptText      string
	ResponseText    string
	ToolCalls       []map[string]any
	ReasoningText   string
	CapturedAt      string
	DurationMs      int64
	FirstResponseMs int64
	ChangedFiles    []string
	RelevantFiles   []string
}

// ParsedTranscript is the result of parsing a VS Code transcript JSONL.
type ParsedTranscript struct {
	SessionID      string
	CopilotVersion string
	VSCodeVersion  string
	StartTime      string
	Exchanges      []ParsedExchange
}

// ParseTranscript parses VS Code Copilot Chat transcript JSONL content into
// structured exchanges.
func ParseTranscript(content string) ParsedTranscript {
	lines := strings.Split(strings.TrimSpace(content), "\n")
	var events []transcriptEvent
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var ev transcriptEvent
		if err := json.Unmarshal([]byte(line), &ev); err != nil {
			continue
		}
		events = append(events, ev)
	}

	result := ParsedTranscript{}

	// Parse session.start
	for _, ev := range events {
		if ev.Type == "session.start" {
			var d sessionStartData
			if err := json.Unmarshal(ev.Data, &d); err == nil {
				result.SessionID = d.SessionID
				result.CopilotVersion = d.CopilotVersion
				result.VSCodeVersion = d.VSCodeVersion
				result.StartTime = d.StartTime
			}
			break
		}
	}

	// Build exchanges: each user.message starts a new exchange, completed by
	// the next user.message or end of events.
	type pendingExchange struct {
		promptText    string
		promptTime    time.Time
		promptTimeStr string
		responses     []string
		reasoning     []string
		toolCalls     []map[string]any
		toolStarts    map[string]time.Time // toolCallID → start time
		firstResponse time.Time
		turnStarts    map[string]time.Time // turnID → start
		turnEnds      map[string]time.Time // turnID → end
	}

	var current *pendingExchange

	finalize := func(pe *pendingExchange) *ParsedExchange {
		if pe == nil || pe.promptText == "" {
			return nil
		}
		ex := &ParsedExchange{
			PromptText:   pe.promptText,
			ResponseText: strings.Join(pe.responses, "\n"),
			ToolCalls:    pe.toolCalls,
			CapturedAt:   pe.promptTimeStr,
		}
		if pe.reasoning != nil {
			ex.ReasoningText = strings.Join(pe.reasoning, "\n")
		}

		// Duration: sum of all turn durations
		var totalDuration int64
		for turnID, start := range pe.turnStarts {
			if end, ok := pe.turnEnds[turnID]; ok {
				totalDuration += end.Sub(start).Milliseconds()
			}
		}
		if totalDuration > 0 {
			ex.DurationMs = totalDuration
		}

		// First response latency
		if !pe.firstResponse.IsZero() && !pe.promptTime.IsZero() {
			ex.FirstResponseMs = pe.firstResponse.Sub(pe.promptTime).Milliseconds()
		}

		// Extract changed/relevant files from tool calls
		ex.ChangedFiles = extractChangedFiles(pe.toolCalls)
		ex.RelevantFiles = extractRelevantFiles(pe.toolCalls)

		return ex
	}

	for _, ev := range events {
		switch ev.Type {
		case "user.message":
			// Finalize previous exchange
			if current != nil {
				if ex := finalize(current); ex != nil {
					result.Exchanges = append(result.Exchanges, *ex)
				}
			}
			var d userMessageData
			if err := json.Unmarshal(ev.Data, &d); err != nil {
				continue
			}
			t := parseTimestamp(ev.Timestamp)
			current = &pendingExchange{
				promptText:    d.Content,
				promptTime:    t,
				promptTimeStr: ev.Timestamp,
				toolStarts:    map[string]time.Time{},
				turnStarts:    map[string]time.Time{},
				turnEnds:      map[string]time.Time{},
			}

		case "assistant.message":
			if current == nil {
				continue
			}
			var d assistantMessageData
			if err := json.Unmarshal(ev.Data, &d); err != nil {
				continue
			}
			if d.Content != "" {
				current.responses = append(current.responses, d.Content)
			}
			if d.ReasoningText != "" {
				current.reasoning = append(current.reasoning, d.ReasoningText)
			}
			if current.firstResponse.IsZero() && (d.Content != "" || len(d.ToolRequests) > 0) {
				current.firstResponse = parseTimestamp(ev.Timestamp)
			}
			// Extract tool calls from toolRequests
			for _, tr := range d.ToolRequests {
				tc := map[string]any{
					"tool": tr.Name,
					"id":   tr.ToolCallID,
				}
				// Parse arguments JSON string into structured input
				if tr.Arguments != "" {
					var args map[string]any
					if err := json.Unmarshal([]byte(tr.Arguments), &args); err == nil {
						tc["input"] = args
					} else {
						tc["input"] = map[string]any{"raw": tr.Arguments}
					}
				}
				current.toolCalls = append(current.toolCalls, tc)
			}

		case "assistant.turn_start":
			if current == nil {
				continue
			}
			var d turnData
			if err := json.Unmarshal(ev.Data, &d); err == nil {
				current.turnStarts[d.TurnID] = parseTimestamp(ev.Timestamp)
			}

		case "assistant.turn_end":
			if current == nil {
				continue
			}
			var d turnData
			if err := json.Unmarshal(ev.Data, &d); err == nil {
				current.turnEnds[d.TurnID] = parseTimestamp(ev.Timestamp)
			}

		case "tool.execution_start":
			if current == nil {
				continue
			}
			var d toolExecStartData
			if err := json.Unmarshal(ev.Data, &d); err == nil {
				current.toolStarts[d.ToolCallID] = parseTimestamp(ev.Timestamp)
			}
		}
	}

	// Finalize last exchange
	if current != nil {
		if ex := finalize(current); ex != nil {
			result.Exchanges = append(result.Exchanges, *ex)
		}
	}

	return result
}

// ExchangeToPromptRecord converts a parsed exchange to a PromptRecord.
func ExchangeToPromptRecord(ex ParsedExchange, sessionID, projectName, projectID, branch string) supabase.PromptRecord {
	fileContext := map[string]any{
		"capture_schema": versions.CaptureSchemaVersion,
		"is_agentic":     len(ex.ToolCalls) > 0,
	}
	if ex.DurationMs > 0 {
		fileContext["response_duration_ms"] = ex.DurationMs
	}
	if ex.FirstResponseMs > 0 {
		fileContext["first_response_ms"] = ex.FirstResponseMs
	}
	if ex.ReasoningText != "" {
		fileContext["reasoning_text"] = ex.ReasoningText
	}
	if len(ex.ChangedFiles) > 0 {
		fileContext["changed_files"] = ex.ChangedFiles
	}
	if len(ex.RelevantFiles) > 0 {
		fileContext["relevant_files"] = ex.RelevantFiles
	}

	capturedAt := ex.CapturedAt
	if capturedAt == "" {
		capturedAt = time.Now().UTC().Format(time.RFC3339)
	}

	return supabase.PromptRecord{
		SessionID:     sessionID,
		ProjectName:   projectName,
		ProjectID:     projectID,
		BranchName:    branch,
		PromptText:    ex.PromptText,
		ResponseText:  ex.ResponseText,
		Source:        "vscode",
		CaptureMethod: "file-watcher",
		ToolCalls:     ex.ToolCalls,
		FileContext:   fileContext,
		CapturedAt:    capturedAt,
	}
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

func parseTimestamp(s string) time.Time {
	if s == "" {
		return time.Time{}
	}
	// Try ISO 8601 with milliseconds
	t, err := time.Parse("2006-01-02T15:04:05.000Z", s)
	if err == nil {
		return t
	}
	t, err = time.Parse("2006-01-02T15:04:05.999Z", s)
	if err == nil {
		return t
	}
	t, err = time.Parse(time.RFC3339, s)
	if err == nil {
		return t
	}
	t, err = time.Parse(time.RFC3339Nano, s)
	if err == nil {
		return t
	}
	return time.Time{}
}

// writeTools are tool names that modify files.
var writeTools = map[string]bool{
	"write_file":                    true,
	"create_file":                   true,
	"edit_file":                     true,
	"replace_string_in_file":        true,
	"multi_replace_string_in_file":  true,
	"edit_notebook_file":            true,
	"create_directory":              true,
}

// readTools are tool names that read files.
var readTools = map[string]bool{
	"read_file":        true,
	"view_image":       true,
	"semantic_search":  true,
}

// extractChangedFiles returns file paths from write-oriented tool calls.
func extractChangedFiles(toolCalls []map[string]any) []string {
	seen := map[string]bool{}
	var files []string
	for _, tc := range toolCalls {
		tool, _ := tc["tool"].(string)
		if !writeTools[tool] {
			continue
		}
		path := pathFromToolInput(tc)
		if path != "" && !seen[path] {
			seen[path] = true
			files = append(files, path)
		}
	}
	return files
}

// extractRelevantFiles returns file paths from read-oriented tool calls.
func extractRelevantFiles(toolCalls []map[string]any) []string {
	seen := map[string]bool{}
	var files []string
	for _, tc := range toolCalls {
		tool, _ := tc["tool"].(string)
		if !readTools[tool] {
			continue
		}
		path := pathFromToolInput(tc)
		if path != "" && !seen[path] {
			seen[path] = true
			files = append(files, path)
		}
	}
	return files
}

// pathFromToolInput extracts the file path from a tool call's input map.
func pathFromToolInput(tc map[string]any) string {
	input, ok := tc["input"].(map[string]any)
	if !ok {
		return ""
	}
	for _, key := range []string{"filePath", "path", "file_path"} {
		if p, ok := input[key].(string); ok && p != "" {
			return p
		}
	}
	return ""
}
