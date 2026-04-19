package tests

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/pcr-developers/cli/internal/sources/claudecode"
)

func msg(typ, sessionID string, content any, extra ...map[string]any) string {
	m := map[string]any{
		"type":       typ,
		"session_id": sessionID,
		"timestamp":  "2024-01-01T00:00:00.000Z",
		"message": map[string]any{
			"content": content,
		},
	}
	for _, e := range extra {
		for k, v := range e {
			m[k] = v
		}
	}
	b, _ := json.Marshal(m)
	return string(b)
}

func textContent(text string) []map[string]any {
	return []map[string]any{{"type": "text", "text": text}}
}

func toolUseContent(id, name string, input map[string]any) []map[string]any {
	return []map[string]any{
		{"type": "text", "text": "I'll call a tool"},
		{"type": "tool_use", "id": id, "name": name, "input": input},
	}
}

func toolResultContent(toolUseID, result string) []map[string]any {
	return []map[string]any{{"type": "tool_result", "tool_use_id": toolUseID, "content": result}}
}

func toolResultWithTextContent(toolUseID, result, text string) []map[string]any {
	return []map[string]any{
		{"type": "tool_result", "tool_use_id": toolUseID, "content": result},
		{"type": "text", "text": text},
	}
}

func buildJSONL(lines []string) string {
	return strings.Join(lines, "\n")
}

// TestResponseNotCutOff_SimpleToolExchange verifies a basic tool-use exchange
// captures the full response text including text after tool results.
func TestResponseNotCutOff_SimpleToolExchange(t *testing.T) {
	sid := "sess-1"
	jsonl := buildJSONL([]string{
		msg("human", sid, textContent("fix the bug")),
		msg("assistant", sid, toolUseContent("tc1", "Read", map[string]any{"path": "/p/f.go"})),
		msg("human", sid, toolResultContent("tc1", "file contents here")),
		msg("assistant", sid, textContent("Here is the fix I applied.")),
	})

	session := claudecode.ParseClaudeCodeSession(jsonl, "proj", "/fake/path.jsonl")
	if len(session.Prompts) != 1 {
		t.Fatalf("expected 1 prompt, got %d", len(session.Prompts))
	}
	resp := session.Prompts[0].ResponseText
	if !strings.Contains(resp, "I'll call a tool") {
		t.Errorf("response missing first text segment; got: %q", resp)
	}
	if !strings.Contains(resp, "Here is the fix I applied.") {
		t.Errorf("response missing text after tool result (cut off); got: %q", resp)
	}
}

// TestResponseNotCutOff_ToolResultWithAutoAcceptText is the main regression test.
// When a human/tool_result message also carries a text block (e.g. auto-accept
// approval annotation), the loop must NOT break — subsequent assistant text must
// still be captured.
func TestResponseNotCutOff_ToolResultWithAutoAcceptText(t *testing.T) {
	sid := "sess-2"
	jsonl := buildJSONL([]string{
		msg("human", sid, textContent("ok do the diff again")),
		msg("assistant", sid, toolUseContent("tc1", "Edit", map[string]any{"path": "/p/push.go"})),
		// Human message: tool_result AND text (auto-accept annotation) — old code broke here
		msg("human", sid, toolResultWithTextContent("tc1", "edit applied", "auto-accepted")),
		// Assistant second segment — must be included in response
		msg("assistant", sid, textContent("Two changes made to push.go.")),
	})

	session := claudecode.ParseClaudeCodeSession(jsonl, "proj", "/fake/path.jsonl")
	if len(session.Prompts) != 1 {
		t.Fatalf("expected 1 prompt, got %d", len(session.Prompts))
	}
	resp := session.Prompts[0].ResponseText
	if !strings.Contains(resp, "Two changes made to push.go.") {
		t.Errorf("response was cut off at auto-accept annotation; got: %q", resp)
	}
}

// TestResponseNotCutOff_MultipleToolRounds verifies multi-turn tool exchanges
// (read → edit → read → edit) all contribute to response text.
func TestResponseNotCutOff_MultipleToolRounds(t *testing.T) {
	sid := "sess-3"
	jsonl := buildJSONL([]string{
		msg("human", sid, textContent("refactor the auth module")),
		msg("assistant", sid, toolUseContent("tc1", "Read", map[string]any{"path": "/p/auth.go"})),
		msg("human", sid, toolResultContent("tc1", "package auth...")),
		msg("assistant", sid, toolUseContent("tc2", "Edit", map[string]any{"path": "/p/auth.go"})),
		msg("human", sid, toolResultContent("tc2", "edit ok")),
		msg("assistant", sid, textContent("Refactoring complete.")),
	})

	session := claudecode.ParseClaudeCodeSession(jsonl, "proj", "/fake/path.jsonl")
	if len(session.Prompts) != 1 {
		t.Fatalf("expected 1 prompt, got %d", len(session.Prompts))
	}
	resp := session.Prompts[0].ResponseText
	if !strings.Contains(resp, "Refactoring complete.") {
		t.Errorf("response missing final text after multi-round tool use; got: %q", resp)
	}
}

// TestResponseBoundary_TwoPrompts verifies the inner loop correctly stops at
// the second human prompt and does not bleed into the next exchange.
func TestResponseBoundary_TwoPrompts(t *testing.T) {
	sid := "sess-4"
	jsonl := buildJSONL([]string{
		msg("human", sid, textContent("first prompt")),
		msg("assistant", sid, textContent("first response")),
		msg("human", sid, textContent("second prompt")),
		msg("assistant", sid, textContent("second response")),
	})

	session := claudecode.ParseClaudeCodeSession(jsonl, "proj", "/fake/path.jsonl")
	if len(session.Prompts) != 2 {
		t.Fatalf("expected 2 prompts, got %d", len(session.Prompts))
	}
	if session.Prompts[0].ResponseText != "first response" {
		t.Errorf("first prompt response wrong: %q", session.Prompts[0].ResponseText)
	}
	if session.Prompts[1].ResponseText != "second response" {
		t.Errorf("second prompt response wrong: %q", session.Prompts[1].ResponseText)
	}
}

// TestPermissionMode_MidExchangeChange verifies that a permission mode change
// on an assistant message during an exchange is captured in PermissionMode.
func TestPermissionMode_MidExchangeChange(t *testing.T) {
	sid := "sess-5"

	assistantWithMode := func(text, mode string) string {
		m := map[string]any{
			"type":           "assistant",
			"session_id":     sid,
			"timestamp":      "2024-01-01T00:00:00.000Z",
			"permissionMode": mode,
			"message": map[string]any{
				"content": textContent(text),
			},
		}
		b, _ := json.Marshal(m)
		return string(b)
	}

	jsonl := buildJSONL([]string{
		msg("human", sid, textContent("do the thing"), map[string]any{"permissionMode": "default"}),
		assistantWithMode("Starting task", "default"),
		msg("human", sid, toolResultContent("tc1", "result")),
		assistantWithMode("Done.", "acceptEdits"),
	})

	session := claudecode.ParseClaudeCodeSession(jsonl, "proj", "/fake/path.jsonl")
	if len(session.Prompts) != 1 {
		t.Fatalf("expected 1 prompt, got %d", len(session.Prompts))
	}
	mode := session.Prompts[0].PermissionMode
	if !strings.Contains(mode, "acceptEdits") {
		t.Errorf("mid-exchange permission mode change not captured; got: %q", mode)
	}
	fmt.Printf("  PermissionMode: %q\n", mode)
}
