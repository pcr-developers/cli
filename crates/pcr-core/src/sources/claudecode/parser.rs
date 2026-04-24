//! Claude Code JSONL transcript parser. Direct port of
//! `cli/internal/sources/claudecode/parser.go`.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

use crate::supabase::PromptRecord;
use crate::util::time::now_rfc3339;

// ─── JSONL message types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct MessageUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    #[allow(dead_code)]
    cache_creation_input_tokens: i64,
    #[serde(default)]
    #[allow(dead_code)]
    cache_read_input_tokens: i64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ContentBlock {
    #[serde(rename = "type")]
    ty: String,
    text: String,
    thinking: String,
    id: String,
    name: String,
    input: Option<Value>,
    tool_use_id: String,
    content: Option<Value>,
    is_error: bool,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct MessageBody {
    role: String,
    content: Option<Value>,
    model: String,
    usage: Option<MessageUsage>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ClaudeMessage {
    #[serde(rename = "type")]
    ty: String,
    message: MessageBody,
    timestamp: String,
    session_id: String,
    #[serde(rename = "sessionId")]
    session_id_camel: String,
    #[serde(rename = "isSidechain")]
    is_sidechain: bool,
    #[serde(rename = "gitBranch")]
    git_branch: String,
    #[serde(rename = "permissionMode")]
    permission_mode: String,
}

/// Result of parsing a single Claude Code JSONL file.
#[derive(Debug, Default)]
pub struct ParsedSession {
    pub session_id: String,
    pub project_name: String,
    pub branch: String,
    pub model: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub exchange_count: usize,
    pub session_created_at: String,
    pub session_updated_at: String,
    pub prompts: Vec<PromptRecord>,
}

/// Parse a full JSONL transcript into structured prompt records. Matches
/// `claudecode.ParseClaudeCodeSession`.
pub fn parse_claude_code_session(
    file_content: &str,
    project_name: &str,
    file_path: &str,
) -> ParsedSession {
    let mut messages: Vec<ClaudeMessage> = Vec::new();
    for line in file_content.trim().split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<ClaudeMessage>(line) else {
            continue;
        };
        if msg.is_sidechain {
            continue;
        }
        if !msg.ty.is_empty() {
            messages.push(msg);
        }
    }

    // Branch from system or first user message.
    let mut branch = String::new();
    for m in &messages {
        if m.ty == "system" && !m.git_branch.is_empty() {
            branch = m.git_branch.clone();
            break;
        }
    }
    if branch.is_empty() {
        for m in &messages {
            if (m.ty == "human" || m.ty == "user") && !m.git_branch.is_empty() {
                branch = m.git_branch.clone();
                break;
            }
        }
    }

    // Session ID: message field > filename stem > timestamp fallback.
    let mut session_id = String::new();
    if let Some(first) = messages.first() {
        session_id = if !first.session_id.is_empty() {
            first.session_id.clone()
        } else {
            first.session_id_camel.clone()
        };
    }
    if session_id.is_empty() {
        if let Some(name) = std::path::Path::new(file_path)
            .file_name()
            .and_then(|s| s.to_str())
        {
            session_id = name.trim_end_matches(".jsonl").to_string();
        }
    }
    if session_id.is_empty() {
        session_id = format!("session-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
    }

    let mut prompts: Vec<PromptRecord> = Vec::new();
    let mut total_input = 0i64;
    let mut total_output = 0i64;
    let mut session_model = String::new();
    let mut session_created_at = String::new();
    let mut session_updated_at = String::new();

    for i in 0..messages.len() {
        let msg = &messages[i];
        if !msg.timestamp.is_empty() {
            if session_created_at.is_empty() {
                session_created_at = msg.timestamp.clone();
            }
            session_updated_at = msg.timestamp.clone();
        }
        if msg.ty != "human" && msg.ty != "user" {
            continue;
        }
        let prompt_text = extract_text(msg.message.content.as_ref());
        if prompt_text.trim().is_empty() {
            continue;
        }

        let mut response_text = String::new();
        let mut model = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut tool_results: Vec<Value> = Vec::new();
        let mut thinking_content = String::new();
        let mut input_tokens = 0i64;
        let mut output_tokens = 0i64;
        let mut agent_tool_ids: HashSet<String> = HashSet::new();

        for j in (i + 1)..messages.len() {
            let next = &messages[j];
            if next.ty == "assistant" {
                let chunk = extract_text(next.message.content.as_ref());
                if !chunk.is_empty() {
                    if !response_text.is_empty() {
                        response_text.push('\n');
                    }
                    response_text.push_str(&chunk);
                }
                let t = extract_thinking(next.message.content.as_ref());
                if !t.is_empty() {
                    if !thinking_content.is_empty() {
                        thinking_content.push('\n');
                    }
                    thinking_content.push_str(&t);
                }
                if model.is_empty() && !next.message.model.is_empty() {
                    model = next.message.model.clone();
                    if session_model.is_empty() {
                        session_model = model.clone();
                    }
                }
                let tools = extract_tool_calls(next.message.content.as_ref());
                for tc in &tools {
                    if tc.get("tool").and_then(|v| v.as_str()) == Some("Agent") {
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                            if !id.is_empty() {
                                agent_tool_ids.insert(id.to_string());
                            }
                        }
                    }
                }
                tool_calls.extend(tools);

                if let Some(usage) = &next.message.usage {
                    input_tokens += usage.input_tokens;
                    output_tokens += usage.output_tokens;
                    total_input += usage.input_tokens;
                    total_output += usage.output_tokens;
                }
                continue;
            }
            if next.ty == "human" || next.ty == "user" {
                if !agent_tool_ids.is_empty() {
                    let agent_text =
                        extract_agent_results(next.message.content.as_ref(), &agent_tool_ids);
                    if !agent_text.is_empty() {
                        if !response_text.is_empty() {
                            response_text.push('\n');
                        }
                        response_text.push_str(&agent_text);
                    }
                }
                let results = extract_tool_results(next.message.content.as_ref());
                tool_results.extend(results);
                if !extract_text(next.message.content.as_ref())
                    .trim()
                    .is_empty()
                {
                    break;
                }
                continue;
            }
            if next.ty == "system" {
                continue;
            }
        }

        let mut file_context = serde_json::Map::new();
        if !tool_results.is_empty() {
            file_context.insert("tool_results".into(), Value::Array(tool_results));
        }
        if !thinking_content.is_empty() {
            file_context.insert("thinking_content".into(), Value::String(thinking_content));
        }
        if input_tokens > 0 {
            file_context.insert("input_tokens".into(), Value::Number(input_tokens.into()));
        }
        if output_tokens > 0 {
            file_context.insert("output_tokens".into(), Value::Number(output_tokens.into()));
        }

        let captured_at = if msg.timestamp.is_empty() {
            now_rfc3339()
        } else {
            msg.timestamp.clone()
        };

        prompts.push(PromptRecord {
            session_id: session_id.clone(),
            project_name: project_name.to_string(),
            branch_name: branch.clone(),
            prompt_text,
            response_text,
            model,
            source: "claude-code".into(),
            capture_method: "file-watcher".into(),
            tool_calls,
            file_context: if file_context.is_empty() {
                None
            } else {
                Some(file_context)
            },
            captured_at,
            permission_mode: msg.permission_mode.clone(),
            ..Default::default()
        });
    }

    let exchange_count = prompts.len();
    ParsedSession {
        session_id,
        project_name: project_name.to_string(),
        branch,
        model: session_model,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        exchange_count,
        session_created_at,
        session_updated_at,
        prompts,
    }
}

// ─── Content extraction helpers ──────────────────────────────────────────────

fn parse_content(raw: Option<&Value>) -> Vec<ContentBlock> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    if let Some(arr) = raw.as_array() {
        let mut blocks = Vec::with_capacity(arr.len());
        for v in arr {
            if let Ok(b) = serde_json::from_value::<ContentBlock>(v.clone()) {
                blocks.push(b);
            }
        }
        return blocks;
    }
    if let Some(s) = raw.as_str() {
        return vec![ContentBlock {
            ty: "text".into(),
            text: s.to_string(),
            ..Default::default()
        }];
    }
    Vec::new()
}

fn extract_text(raw: Option<&Value>) -> String {
    parse_content(raw)
        .into_iter()
        .filter(|b| b.ty == "text" && !b.text.is_empty())
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_thinking(raw: Option<&Value>) -> String {
    parse_content(raw)
        .into_iter()
        .filter(|b| b.ty == "thinking" && !b.thinking.is_empty())
        .map(|b| b.thinking)
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_tool_calls(raw: Option<&Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for b in parse_content(raw) {
        if b.ty == "tool_use" {
            let mut m = serde_json::Map::new();
            m.insert("tool".into(), Value::String(b.name));
            m.insert(
                "input".into(),
                b.input.unwrap_or(Value::Object(serde_json::Map::new())),
            );
            m.insert("id".into(), Value::String(b.id));
            out.push(Value::Object(m));
        }
    }
    out
}

fn extract_agent_results(raw: Option<&Value>, agent_ids: &HashSet<String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in parse_content(raw) {
        if b.ty != "tool_result" || !agent_ids.contains(&b.tool_use_id) {
            continue;
        }
        let text = content_to_text(b.content.as_ref());
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts.join("\n")
}

fn extract_tool_results(raw: Option<&Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for b in parse_content(raw) {
        if b.ty != "tool_result" {
            continue;
        }
        let mut text = content_to_text(b.content.as_ref());
        if text.chars().count() > 500 {
            let truncated: String = text.chars().take(497).collect();
            text = format!("{truncated}…");
        }
        let mut m = serde_json::Map::new();
        m.insert("tool_use_id".into(), Value::String(b.tool_use_id));
        m.insert("content".into(), Value::String(text));
        m.insert("is_error".into(), Value::Bool(b.is_error));
        out.push(Value::Object(m));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_user_assistant_pair() {
        let jsonl = r#"{"type":"user","timestamp":"2026-04-24T00:00:00Z","session_id":"s1","message":{"role":"user","content":"fix the bug"}}
{"type":"assistant","timestamp":"2026-04-24T00:00:01Z","session_id":"s1","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":3}}}"#;
        let session = parse_claude_code_session(jsonl, "proj", "/tmp/s1.jsonl");
        assert_eq!(session.session_id, "s1");
        assert_eq!(session.exchange_count, 1);
        assert_eq!(session.prompts[0].prompt_text, "fix the bug");
        assert_eq!(session.prompts[0].response_text, "done");
        assert_eq!(session.prompts[0].model, "claude-sonnet-4-6");
        assert_eq!(session.total_input_tokens, 10);
        assert_eq!(session.total_output_tokens, 3);
    }

    #[test]
    fn tool_use_block_becomes_tool_call() {
        let jsonl = r#"{"type":"user","timestamp":"2026-04-24T00:00:00Z","session_id":"s1","message":{"role":"user","content":"go"}}
{"type":"assistant","timestamp":"2026-04-24T00:00:01Z","session_id":"s1","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Write","input":{"path":"/x.rs"}}]}}"#;
        let session = parse_claude_code_session(jsonl, "proj", "/tmp/s1.jsonl");
        assert_eq!(session.prompts.len(), 1);
        let tcs = &session.prompts[0].tool_calls;
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].get("tool").and_then(|v| v.as_str()), Some("Write"));
        assert_eq!(tcs[0].get("id").and_then(|v| v.as_str()), Some("t1"));
    }

    #[test]
    fn sidechain_messages_skipped() {
        let jsonl = r#"{"type":"user","timestamp":"2026-04-24T00:00:00Z","session_id":"s1","isSidechain":true,"message":{"role":"user","content":"ignored"}}
{"type":"user","timestamp":"2026-04-24T00:00:01Z","session_id":"s1","message":{"role":"user","content":"kept"}}"#;
        let session = parse_claude_code_session(jsonl, "proj", "/tmp/s1.jsonl");
        assert_eq!(session.prompts.len(), 1);
        assert_eq!(session.prompts[0].prompt_text, "kept");
    }
}

fn content_to_text(raw: Option<&Value>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    if let Some(s) = raw.as_str() {
        return s.to_string();
    }
    if let Some(arr) = raw.as_array() {
        let mut parts: Vec<String> = Vec::new();
        for v in arr {
            if let Ok(sb) = serde_json::from_value::<ContentBlock>(v.clone()) {
                if sb.ty == "text" && !sb.text.is_empty() {
                    parts.push(sb.text);
                }
            }
        }
        return parts.join("\n");
    }
    String::new()
}
