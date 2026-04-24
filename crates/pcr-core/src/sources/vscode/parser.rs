//! VS Code Copilot Chat transcript parser. Direct port of
//! `cli/internal/sources/vscode/parser.go`.

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::supabase::PromptRecord;
use crate::util::time::now_rfc3339;
use crate::versions;

#[derive(Debug, Deserialize)]
#[serde(default)]
struct TranscriptEvent {
    #[serde(rename = "type")]
    ty: String,
    data: Value,
    #[allow(dead_code)]
    id: String,
    timestamp: String,
    #[serde(rename = "parentId")]
    #[allow(dead_code)]
    parent_id: Option<String>,
}

impl Default for TranscriptEvent {
    fn default() -> Self {
        Self {
            ty: String::new(),
            data: Value::Null,
            id: String::new(),
            timestamp: String::new(),
            parent_id: None,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SessionStartData {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "copilotVersion")]
    copilot_version: String,
    #[serde(rename = "vscodeVersion")]
    vscode_version: String,
    #[serde(rename = "startTime")]
    #[allow(dead_code)]
    start_time: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ToolRequest {
    #[serde(rename = "toolCallId")]
    tool_call_id: String,
    name: String,
    arguments: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    ty: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct AssistantMessageData {
    #[serde(rename = "messageId")]
    #[allow(dead_code)]
    message_id: String,
    content: String,
    #[serde(rename = "toolRequests")]
    tool_requests: Vec<ToolRequest>,
    #[serde(rename = "reasoningText")]
    reasoning_text: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct UserMessageData {
    content: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct TurnData {
    #[serde(rename = "turnId")]
    turn_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedExchange {
    pub prompt_text: String,
    pub response_text: String,
    pub tool_calls: Vec<Value>,
    pub reasoning_text: String,
    pub captured_at: String,
    pub duration_ms: i64,
    pub first_response_ms: i64,
    pub changed_files: Vec<String>,
    pub relevant_files: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ParsedTranscript {
    pub session_id: String,
    pub copilot_version: String,
    pub vscode_version: String,
    pub start_time: String,
    pub exchanges: Vec<ParsedExchange>,
}

/// Parse VS Code Copilot Chat transcript JSONL content into structured
/// exchanges. Matches `vscode.ParseTranscript`.
pub fn parse_transcript(content: &str) -> ParsedTranscript {
    let mut events: Vec<TranscriptEvent> = Vec::new();
    for line in content.trim().split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<TranscriptEvent>(line) {
            events.push(ev);
        }
    }

    let mut result = ParsedTranscript::default();

    for ev in &events {
        if ev.ty == "session.start" {
            if let Ok(d) = serde_json::from_value::<SessionStartData>(ev.data.clone()) {
                result.session_id = d.session_id;
                result.copilot_version = d.copilot_version;
                result.vscode_version = d.vscode_version;
                result.start_time = ev.timestamp.clone();
            }
            break;
        }
    }

    struct Pending {
        prompt_text: String,
        prompt_time: Option<DateTime<Utc>>,
        prompt_time_str: String,
        responses: Vec<String>,
        reasoning: Vec<String>,
        tool_calls: Vec<Value>,
        first_response: Option<DateTime<Utc>>,
        turn_starts: HashMap<String, DateTime<Utc>>,
        turn_ends: HashMap<String, DateTime<Utc>>,
    }

    fn finalize(pe: Pending) -> Option<ParsedExchange> {
        if pe.prompt_text.is_empty() {
            return None;
        }
        let mut ex = ParsedExchange {
            prompt_text: pe.prompt_text,
            response_text: pe.responses.join("\n"),
            tool_calls: pe.tool_calls.clone(),
            captured_at: pe.prompt_time_str,
            ..Default::default()
        };
        if !pe.reasoning.is_empty() {
            ex.reasoning_text = pe.reasoning.join("\n");
        }
        let mut total_duration = 0i64;
        for (turn_id, start) in &pe.turn_starts {
            if let Some(end) = pe.turn_ends.get(turn_id) {
                let delta = (*end - *start).num_milliseconds();
                if delta > 0 {
                    total_duration += delta;
                }
            }
        }
        if total_duration > 0 {
            ex.duration_ms = total_duration;
        }
        if let (Some(start), Some(first)) = (pe.prompt_time, pe.first_response) {
            let delta = (first - start).num_milliseconds();
            if delta > 0 {
                ex.first_response_ms = delta;
            }
        }
        ex.changed_files = extract_changed_files(&pe.tool_calls);
        ex.relevant_files = extract_relevant_files(&pe.tool_calls);
        Some(ex)
    }

    let mut current: Option<Pending> = None;

    for ev in events {
        match ev.ty.as_str() {
            "user.message" => {
                if let Some(pe) = current.take() {
                    if let Some(ex) = finalize(pe) {
                        result.exchanges.push(ex);
                    }
                }
                let Ok(d) = serde_json::from_value::<UserMessageData>(ev.data.clone()) else {
                    continue;
                };
                let t = parse_timestamp(&ev.timestamp);
                current = Some(Pending {
                    prompt_text: d.content,
                    prompt_time: t,
                    prompt_time_str: ev.timestamp.clone(),
                    responses: Vec::new(),
                    reasoning: Vec::new(),
                    tool_calls: Vec::new(),
                    first_response: None,
                    turn_starts: HashMap::new(),
                    turn_ends: HashMap::new(),
                });
            }
            "assistant.message" => {
                let Some(pe) = current.as_mut() else {
                    continue;
                };
                let Ok(d) = serde_json::from_value::<AssistantMessageData>(ev.data.clone()) else {
                    continue;
                };
                if !d.content.is_empty() {
                    pe.responses.push(d.content.clone());
                }
                if !d.reasoning_text.is_empty() {
                    pe.reasoning.push(d.reasoning_text);
                }
                if pe.first_response.is_none()
                    && (!d.content.is_empty() || !d.tool_requests.is_empty())
                {
                    pe.first_response = parse_timestamp(&ev.timestamp);
                }
                for tr in d.tool_requests {
                    let mut tc = serde_json::Map::new();
                    tc.insert("tool".into(), Value::String(tr.name.clone()));
                    tc.insert("id".into(), Value::String(tr.tool_call_id.clone()));
                    if !tr.arguments.is_empty() {
                        match serde_json::from_str::<Value>(&tr.arguments) {
                            Ok(args) => {
                                tc.insert("input".into(), args);
                            }
                            Err(_) => {
                                let mut raw = serde_json::Map::new();
                                raw.insert("raw".into(), Value::String(tr.arguments));
                                tc.insert("input".into(), Value::Object(raw));
                            }
                        }
                    }
                    pe.tool_calls.push(Value::Object(tc));
                }
            }
            "assistant.turn_start" => {
                let Some(pe) = current.as_mut() else {
                    continue;
                };
                if let Ok(d) = serde_json::from_value::<TurnData>(ev.data.clone()) {
                    if let Some(t) = parse_timestamp(&ev.timestamp) {
                        pe.turn_starts.insert(d.turn_id, t);
                    }
                }
            }
            "assistant.turn_end" => {
                let Some(pe) = current.as_mut() else {
                    continue;
                };
                if let Ok(d) = serde_json::from_value::<TurnData>(ev.data.clone()) {
                    if let Some(t) = parse_timestamp(&ev.timestamp) {
                        pe.turn_ends.insert(d.turn_id, t);
                    }
                }
            }
            "tool.execution_start" => {
                // Currently only used for per-tool timing — we don't track
                // per-tool durations yet. Matches parity with the Go code
                // which also only stores toolStarts but never reads it.
            }
            _ => {}
        }
    }

    if let Some(pe) = current {
        if let Some(ex) = finalize(pe) {
            result.exchanges.push(ex);
        }
    }

    result
}

/// `ExchangeToPromptRecord`.
pub fn exchange_to_prompt_record(
    ex: &ParsedExchange,
    session_id: &str,
    project_name: &str,
    project_id: &str,
    branch: &str,
) -> PromptRecord {
    let mut file_context = serde_json::Map::new();
    file_context.insert(
        "capture_schema".into(),
        Value::Number(versions::CAPTURE_SCHEMA_VERSION.into()),
    );
    file_context.insert("is_agentic".into(), Value::Bool(!ex.tool_calls.is_empty()));
    if ex.duration_ms > 0 {
        file_context.insert(
            "response_duration_ms".into(),
            Value::Number(ex.duration_ms.into()),
        );
    }
    if ex.first_response_ms > 0 {
        file_context.insert(
            "first_response_ms".into(),
            Value::Number(ex.first_response_ms.into()),
        );
    }
    if !ex.reasoning_text.is_empty() {
        file_context.insert(
            "reasoning_text".into(),
            Value::String(ex.reasoning_text.clone()),
        );
    }
    if !ex.changed_files.is_empty() {
        file_context.insert(
            "changed_files".into(),
            Value::Array(
                ex.changed_files
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !ex.relevant_files.is_empty() {
        file_context.insert(
            "relevant_files".into(),
            Value::Array(
                ex.relevant_files
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }

    let captured_at = if ex.captured_at.is_empty() {
        now_rfc3339()
    } else {
        ex.captured_at.clone()
    };

    PromptRecord {
        session_id: session_id.to_string(),
        project_name: project_name.to_string(),
        project_id: project_id.to_string(),
        branch_name: branch.to_string(),
        prompt_text: ex.prompt_text.clone(),
        response_text: ex.response_text.clone(),
        source: "vscode".into(),
        capture_method: "file-watcher".into(),
        tool_calls: ex.tool_calls.clone(),
        file_context: Some(file_context),
        captured_at,
        ..Default::default()
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.3fZ", "%Y-%m-%dT%H:%M:%S%.fZ"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
        }
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

fn write_tools() -> &'static HashSet<&'static str> {
    static WRITE: std::sync::OnceLock<HashSet<&'static str>> = std::sync::OnceLock::new();
    WRITE.get_or_init(|| {
        [
            "write_file",
            "create_file",
            "edit_file",
            "replace_string_in_file",
            "multi_replace_string_in_file",
            "edit_notebook_file",
            "create_directory",
        ]
        .into_iter()
        .collect()
    })
}

fn read_tools() -> &'static HashSet<&'static str> {
    static READ: std::sync::OnceLock<HashSet<&'static str>> = std::sync::OnceLock::new();
    READ.get_or_init(|| {
        ["read_file", "view_image", "semantic_search"]
            .into_iter()
            .collect()
    })
}

pub fn extract_changed_files(tool_calls: &[Value]) -> Vec<String> {
    let tools = write_tools();
    let mut seen: HashSet<String> = HashSet::new();
    let mut files = Vec::new();
    for tc in tool_calls {
        let tool = tc.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        if !tools.contains(tool) {
            continue;
        }
        if let Some(p) = path_from_tool_input(tc) {
            if seen.insert(p.clone()) {
                files.push(p);
            }
        }
    }
    files
}

pub fn extract_relevant_files(tool_calls: &[Value]) -> Vec<String> {
    let tools = read_tools();
    let mut seen: HashSet<String> = HashSet::new();
    let mut files = Vec::new();
    for tc in tool_calls {
        let tool = tc.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        if !tools.contains(tool) {
            continue;
        }
        if let Some(p) = path_from_tool_input(tc) {
            if seen.insert(p.clone()) {
                files.push(p);
            }
        }
    }
    files
}

fn path_from_tool_input(tc: &Value) -> Option<String> {
    let input = tc.get("input").and_then(|v| v.as_object())?;
    for key in ["filePath", "path", "file_path"] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_start_and_user_assistant() {
        let jsonl = r#"{"type":"session.start","timestamp":"2026-04-24T10:00:00.000Z","data":{"sessionId":"vs1","copilotVersion":"1.2.3","vscodeVersion":"1.90","startTime":"2026-04-24T10:00:00.000Z"}}
{"type":"user.message","timestamp":"2026-04-24T10:00:05.000Z","data":{"content":"ping"}}
{"type":"assistant.message","timestamp":"2026-04-24T10:00:06.000Z","data":{"content":"pong","toolRequests":[{"toolCallId":"c1","name":"read_file","arguments":"{\"filePath\":\"/a.rs\"}"}]}}"#;
        let t = parse_transcript(jsonl);
        assert_eq!(t.session_id, "vs1");
        assert_eq!(t.copilot_version, "1.2.3");
        assert_eq!(t.exchanges.len(), 1);
        assert_eq!(t.exchanges[0].prompt_text, "ping");
        assert_eq!(t.exchanges[0].response_text, "pong");
        assert_eq!(t.exchanges[0].tool_calls.len(), 1);
        assert_eq!(t.exchanges[0].relevant_files, vec!["/a.rs"]);
    }

    #[test]
    fn multiple_user_messages_produce_separate_exchanges() {
        let jsonl = r#"{"type":"user.message","timestamp":"2026-04-24T10:00:00.000Z","data":{"content":"first"}}
{"type":"user.message","timestamp":"2026-04-24T10:00:01.000Z","data":{"content":"second"}}"#;
        let t = parse_transcript(jsonl);
        assert_eq!(t.exchanges.len(), 2);
        assert_eq!(t.exchanges[0].prompt_text, "first");
        assert_eq!(t.exchanges[1].prompt_text, "second");
    }
}
