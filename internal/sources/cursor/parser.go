package cursor

import (
	"encoding/json"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/supabase"
)

type cursorMessage struct {
	Role    string `json:"role"`
	Message struct {
		Content json.RawMessage `json:"content"`
	} `json:"message"`
}

type cursorContentBlock struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

type ParsedCursorSession struct {
	SessionID   string
	ProjectName string
	Prompts     []supabase.PromptRecord
}

// SlugToProjectName converts a Cursor slug to a readable project name.
// "Users-kalujo-Desktop-my-app" → "my-app"
func SlugToProjectName(slug string) string {
	parts := strings.Split(slug, "-")
	known := map[string]bool{
		"Users": true, "home": true, "Desktop": true,
		"Documents": true, "Projects": true, "code": true, "dev": true,
	}
	i := 0
	for i < len(parts) && known[parts[i]] {
		i++
	}
	i++ // skip username
	if i >= len(parts) {
		return slug
	}
	return strings.Join(parts[i:], "-")
}

func extractCursorText(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	// Try string first
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s
	}
	// Try array of content blocks
	var blocks []cursorContentBlock
	if err := json.Unmarshal(raw, &blocks); err == nil {
		var parts []string
		for _, b := range blocks {
			if b.Type == "text" && b.Text != "" {
				parts = append(parts, b.Text)
			}
		}
		return strings.Join(parts, "\n")
	}
	return ""
}

// ParseCursorTranscript parses a Cursor agent-transcript JSONL file.
func ParseCursorTranscript(fileContent, sessionUUID, projectSlug string) ParsedCursorSession {
	projectName := SlugToProjectName(projectSlug)
	lines := strings.Split(strings.TrimSpace(fileContent), "\n")
	var messages []cursorMessage

	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var msg cursorMessage
		if err := json.Unmarshal([]byte(line), &msg); err != nil {
			continue
		}
		if msg.Role != "" {
			messages = append(messages, msg)
		}
	}

	var prompts []supabase.PromptRecord
	for i, msg := range messages {
		if msg.Role != "user" {
			continue
		}
		promptText := extractCursorText(msg.Message.Content)
		if strings.TrimSpace(promptText) == "" {
			continue
		}

		var responseText string
		if i+1 < len(messages) && messages[i+1].Role == "assistant" {
			responseText = extractCursorText(messages[i+1].Message.Content)
		}

		prompts = append(prompts, supabase.PromptRecord{
			SessionID:     sessionUUID,
			ProjectName:   projectName,
			PromptText:    promptText,
			ResponseText:  responseText,
			Source:        "cursor",
			CaptureMethod: "file-watcher",
			CapturedAt:    time.Now().UTC().Format(time.RFC3339),
		})
	}

	return ParsedCursorSession{
		SessionID:   sessionUUID,
		ProjectName: projectName,
		Prompts:     prompts,
	}
}
