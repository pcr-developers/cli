//! VS Code Copilot Chat session parser for the new `chatSessions/`
//! JSONL format introduced in vscode 1.117 / copilot-chat 0.45+.
//!
//! Lines are CRDT-style ops:
//!   `{"kind":0,"v":<full snapshot>}`              — initial state
//!   `{"kind":1,"k":[..path..],"v":<value>}`        — set state[path] = value
//!   `{"kind":2,"k":[..path..],"v":[..items..]}`    — extend array at path
//!   `{"kind":2,"k":[..path..],"v":<single>}`       — set state[path] (rare)
//!
//! We replay every op into one `serde_json::Value`, then walk
//! `requests[]` to produce `ParsedExchange`es matching the legacy
//! `transcripts/` parser's output schema. The legacy parser still owns
//! `ParsedExchange` / `ParsedTranscript` definitions.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;

use crate::sources::vscode::parser::{
    extract_changed_files, extract_relevant_files, ParsedExchange, ParsedTranscript,
};

/// Replay all ops in a chatSessions JSONL into a single `Value`.
fn replay(content: &str) -> Value {
    let mut state = Value::Object(serde_json::Map::new());
    for line in content.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(op) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = op.get("kind").and_then(|v| v.as_i64()).unwrap_or(-1);
        let path = op
            .get("k")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let value = op.get("v").cloned().unwrap_or(Value::Null);
        match kind {
            0 => {
                state = value;
            }
            1 => {
                set_path(&mut state, &path, value);
            }
            2 => {
                if let Value::Array(items) = &value {
                    extend_path(&mut state, &path, items);
                } else {
                    set_path(&mut state, &path, value);
                }
            }
            _ => {}
        }
    }
    state
}

/// Walk `path` into `state`, creating `Object`/`Array` parents as
/// needed, then assign `value` at the final segment.
fn set_path(state: &mut Value, path: &[Value], value: Value) {
    if path.is_empty() {
        *state = value;
        return;
    }
    let mut cur: &mut Value = state;
    for seg in &path[..path.len() - 1] {
        cur = step_mut(cur, seg);
    }
    assign(cur, &path[path.len() - 1], value);
}

fn extend_path(state: &mut Value, path: &[Value], items: &[Value]) {
    if path.is_empty() {
        if let Value::Array(arr) = state {
            arr.extend(items.iter().cloned());
        }
        return;
    }
    let mut cur: &mut Value = state;
    for seg in &path[..path.len() - 1] {
        cur = step_mut(cur, seg);
    }
    let target = step_mut(cur, &path[path.len() - 1]);
    if !matches!(target, Value::Array(_)) {
        *target = Value::Array(Vec::new());
    }
    if let Value::Array(arr) = target {
        arr.extend(items.iter().cloned());
    }
}

/// Descend one step into `cur` along `seg`, creating empty containers
/// when missing or of the wrong type.
fn step_mut<'a>(cur: &'a mut Value, seg: &Value) -> &'a mut Value {
    match seg {
        Value::String(key) => {
            if !matches!(cur, Value::Object(_)) {
                *cur = Value::Object(serde_json::Map::new());
            }
            let map = cur.as_object_mut().expect("ensured object above");
            map.entry(key.clone())
                .or_insert(Value::Object(serde_json::Map::new()))
        }
        Value::Number(n) => {
            let idx = n.as_u64().unwrap_or(0) as usize;
            if !matches!(cur, Value::Array(_)) {
                *cur = Value::Array(Vec::new());
            }
            let arr = cur.as_array_mut().expect("ensured array above");
            while arr.len() <= idx {
                arr.push(Value::Object(serde_json::Map::new()));
            }
            &mut arr[idx]
        }
        _ => cur,
    }
}

fn assign(parent: &mut Value, seg: &Value, value: Value) {
    match seg {
        Value::String(key) => {
            if !matches!(parent, Value::Object(_)) {
                *parent = Value::Object(serde_json::Map::new());
            }
            if let Some(map) = parent.as_object_mut() {
                map.insert(key.clone(), value);
            }
        }
        Value::Number(n) => {
            let idx = n.as_u64().unwrap_or(0) as usize;
            if !matches!(parent, Value::Array(_)) {
                *parent = Value::Array(Vec::new());
            }
            if let Some(arr) = parent.as_array_mut() {
                while arr.len() <= idx {
                    arr.push(Value::Null);
                }
                arr[idx] = value;
            }
        }
        _ => {}
    }
}

/// Convert a Unix-millis timestamp into the same ISO-8601 string the
/// legacy parser uses on captured_at fields.
fn ms_to_iso(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// `parse_chatsession` — public entry. Returns `ParsedTranscript` so
/// callers can use the same downstream `exchange_to_prompt_record`
/// machinery as the legacy transcripts parser.
pub fn parse_chatsession(content: &str) -> ParsedTranscript {
    let state = replay(content);
    let mut out = ParsedTranscript {
        session_id: state
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        ..Default::default()
    };

    let Some(requests) = state.get("requests").and_then(|v| v.as_array()) else {
        return out;
    };

    for req in requests {
        // Skip incomplete requests (no result yet => still streaming).
        let result = req.get("result");
        if result.is_none() {
            continue;
        }
        let prompt_text = req
            .get("message")
            .and_then(|m| m.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if prompt_text.is_empty() {
            continue;
        }

        let timestamp_ms = req.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        let captured_at = if timestamp_ms > 0 {
            ms_to_iso(timestamp_ms)
        } else {
            String::new()
        };

        let mut response_parts: Vec<String> = Vec::new();
        let mut reasoning_parts: Vec<String> = Vec::new();
        if let Some(items) = req.get("response").and_then(|v| v.as_array()) {
            for item in items {
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
                if kind == "thinking" {
                    if !value.is_empty() {
                        reasoning_parts.push(value.to_string());
                    }
                } else if kind.is_empty() {
                    // Markdown response chunks have no `kind` field but do
                    // carry a `value` string and `supportThemeIcons` etc.
                    if !value.is_empty() {
                        response_parts.push(value.to_string());
                    }
                }
                // Other kinds (mcpServersStarting, codeblockUri, ...) are
                // metadata, not user-visible response text.
            }
        }

        let tool_calls = collect_tool_calls(req);

        let duration_ms = req.get("elapsedMs").and_then(|v| v.as_i64()).unwrap_or(0);
        let first_response_ms = result
            .and_then(|r| r.get("timings"))
            .and_then(|t| t.get("firstProgress"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let mut ex = ParsedExchange {
            prompt_text,
            response_text: response_parts.join("\n"),
            tool_calls: tool_calls.clone(),
            reasoning_text: reasoning_parts.join("\n"),
            captured_at,
            duration_ms,
            first_response_ms,
            ..Default::default()
        };
        ex.changed_files = extract_changed_files(&tool_calls);
        ex.relevant_files = extract_relevant_files(&tool_calls);
        merge_codeblock_files(&mut ex, req);
        out.exchanges.push(ex);
    }
    out
}

/// Pull tool calls out of `result.metadata.toolCallRounds[*].toolCalls[*]`
/// and reshape them into the `{tool, id, input}` schema that the legacy
/// parser produces (so `extract_changed_files` etc. work unchanged).
fn collect_tool_calls(req: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let rounds = req
        .get("result")
        .and_then(|r| r.get("metadata"))
        .and_then(|m| m.get("toolCallRounds"))
        .and_then(|v| v.as_array());
    let Some(rounds) = rounds else {
        return out;
    };
    for round in rounds {
        let Some(calls) = round.get("toolCalls").and_then(|v| v.as_array()) else {
            continue;
        };
        for call in calls {
            let mut entry = serde_json::Map::new();
            if let Some(name) = call.get("name").and_then(|v| v.as_str()) {
                entry.insert("tool".into(), Value::String(name.to_string()));
            }
            if let Some(id) = call
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| call.get("toolCallId").and_then(|v| v.as_str()))
            {
                entry.insert("id".into(), Value::String(id.to_string()));
            }
            // arguments may be a JSON object or a stringified blob.
            if let Some(args) = call.get("arguments") {
                let parsed = match args {
                    Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| {
                        let mut raw = serde_json::Map::new();
                        raw.insert("raw".into(), Value::String(s.clone()));
                        Value::Object(raw)
                    }),
                    other => other.clone(),
                };
                entry.insert("input".into(), parsed);
            }
            out.push(Value::Object(entry));
        }
    }
    out
}

/// Augment `changed_files`/`relevant_files` with paths from
/// `result.metadata.codeBlocks[]` and from any `baseUri` references in
/// the response items. These give us project-attribution signals even
/// when no tool calls fire (e.g. pure conversational replies).
fn merge_codeblock_files(ex: &mut ParsedExchange, req: &Value) {
    let mut seen_changed: HashSet<String> = ex.changed_files.iter().cloned().collect();
    let mut seen_relevant: HashSet<String> = ex.relevant_files.iter().cloned().collect();

    if let Some(blocks) = req
        .get("result")
        .and_then(|r| r.get("metadata"))
        .and_then(|m| m.get("codeBlocks"))
        .and_then(|v| v.as_array())
    {
        for b in blocks {
            if let Some(p) = b
                .get("uri")
                .and_then(|u| u.get("path"))
                .and_then(|v| v.as_str())
            {
                let path = strip_leading_slash(p);
                if seen_changed.insert(path.clone()) {
                    ex.changed_files.push(path);
                }
            }
        }
    }

    // baseUri inside response items signals "current workspace context"
    // — treat as relevant_files so attribution still picks it up.
    if let Some(items) = req.get("response").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(p) = item
                .get("baseUri")
                .and_then(|u| u.get("path"))
                .and_then(|v| v.as_str())
            {
                let path = strip_leading_slash(p);
                if seen_relevant.insert(path.clone()) {
                    ex.relevant_files.push(path);
                }
            }
        }
    }
}

/// VS Code URI paths look like `/c:/Users/...` on Windows. Drop the
/// leading slash before the drive letter so downstream path matchers
/// can treat them as plain Windows paths.
fn strip_leading_slash(p: &str) -> String {
    let bytes = p.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
        p[1..].to_string()
    } else {
        p.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_set_and_extend() {
        let jsonl = r#"{"kind":0,"v":{"sessionId":"s1","requests":[]}}
{"kind":2,"k":["requests"],"v":[{"requestId":"r1","timestamp":1000,"message":{"text":"hi"},"response":[],"result":{"metadata":{}}}]}
{"kind":1,"k":["requests",0,"elapsedMs"],"v":42}
{"kind":2,"k":["requests",0,"response"],"v":[{"value":"hello"}]}"#;
        let t = parse_chatsession(jsonl);
        assert_eq!(t.session_id, "s1");
        assert_eq!(t.exchanges.len(), 1);
        assert_eq!(t.exchanges[0].prompt_text, "hi");
        assert_eq!(t.exchanges[0].response_text, "hello");
        assert_eq!(t.exchanges[0].duration_ms, 42);
    }

    #[test]
    fn skips_incomplete_request_without_result() {
        let jsonl = r#"{"kind":0,"v":{"sessionId":"s2","requests":[{"requestId":"r","timestamp":1,"message":{"text":"q"},"response":[]}]}}"#;
        let t = parse_chatsession(jsonl);
        assert_eq!(t.exchanges.len(), 0);
    }

    #[test]
    fn extracts_thinking_as_reasoning() {
        let jsonl = r#"{"kind":0,"v":{"sessionId":"s3","requests":[{"requestId":"r","timestamp":1,"message":{"text":"q"},"response":[{"kind":"thinking","value":"reasoned"},{"value":"answer"}],"result":{"metadata":{}}}]}}"#;
        let t = parse_chatsession(jsonl);
        assert_eq!(t.exchanges.len(), 1);
        assert_eq!(t.exchanges[0].reasoning_text, "reasoned");
        assert_eq!(t.exchanges[0].response_text, "answer");
    }

    #[test]
    fn collects_toolcallrounds() {
        let jsonl = r#"{"kind":0,"v":{"sessionId":"s","requests":[{"requestId":"r","timestamp":1,"message":{"text":"q"},"response":[],"result":{"metadata":{"toolCallRounds":[{"toolCalls":[{"name":"read_file","id":"c1","arguments":"{\"filePath\":\"/x.rs\"}"}]}]}}}]}}"#;
        let t = parse_chatsession(jsonl);
        assert_eq!(t.exchanges[0].tool_calls.len(), 1);
        assert_eq!(t.exchanges[0].relevant_files, vec!["/x.rs"]);
    }
}
