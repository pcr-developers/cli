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
	var text string
	// Try string first
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		text = s
	} else {
		// Try array of content blocks
		var blocks []cursorContentBlock
		if err := json.Unmarshal(raw, &blocks); err == nil {
			var parts []string
			for _, b := range blocks {
				if b.Type == "text" && b.Text != "" {
					parts = append(parts, b.Text)
				}
			}
			text = strings.Join(parts, "\n")
		}
	}
	return extractUserQuery(text)
}

// extractUserQuery extracts the actual human-typed text from a Cursor message.
// Cursor wraps user input in <user_query>...</user_query> and injects system
// context (<attached_files>, <code_selection>, <image_files>, etc.) into the
// same message. We extract only the user_query content, or return "" for
// pure system injections that have no human text.
func extractUserQuery(text string) string {
	text = strings.TrimSpace(text)
	if text == "" {
		return ""
	}

	// Extract content from <user_query>...</user_query> if present
	const openTag = "<user_query>"
	const closeTag = "</user_query>"
	start := strings.Index(text, openTag)
	end := strings.LastIndex(text, closeTag)
	if start != -1 && end > start {
		inner := strings.TrimSpace(text[start+len(openTag) : end])
		if inner != "" {
			return inner
		}
	}

	// No <user_query> tag — check if the whole message is system-injected context.
	// These start with tags like <attached_files>, <image_files>, [Image], etc.
	trimmed := strings.TrimSpace(text)
	systemPrefixes := []string{
		"<attached_files>", "<image_files>", "<system_reminder>",
		"<open_and_recently", "<task_notification>", "[Image]",
		"<agent_transcripts>", "<agent_skills>",
	}
	for _, prefix := range systemPrefixes {
		if strings.HasPrefix(trimmed, prefix) {
			return "" // pure system injection, skip
		}
	}

	// Plain text with no XML wrapping — return as-is (older Cursor format)
	return text
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

		// Offset each prompt by its index so batched captures have distinct
		// timestamps. The watcher will override with bubble.SubmittedAt from
		// Cursor's SQLite DB when available (more accurate).
		capturedAt := time.Now().Add(time.Duration(len(prompts)) * time.Second).UTC().Format(time.RFC3339)

		prompts = append(prompts, supabase.PromptRecord{
			SessionID:     sessionUUID,
			ProjectName:   projectName,
			PromptText:    promptText,
			ResponseText:  responseText,
			Source:        "cursor",
			CaptureMethod: "file-watcher",
			CapturedAt:    capturedAt,
		})
	}

	return ParsedCursorSession{
		SessionID:   sessionUUID,
		ProjectName: projectName,
		Prompts:     prompts,
	}
}
