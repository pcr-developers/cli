//! VS Code "empty window" Copilot chat session processor. Direct port of
//! `cli/internal/sources/vscode/empty_window.go`.
//!
//! Handles both the legacy v3 JSON format and the newer mutation-log JSONL
//! format (kind: 0 = full snapshot; kind: 1 = property mutation).

use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::PathBuf;

use crate::display;
use crate::sources::shared::{Deduplicator, FileState};
use crate::sources::vscode::workspace::global_storage_base;
use crate::store::{self, is_draft_saved_at};
use crate::supabase::{self, PromptRecord};
use crate::versions;

// ─── Types matching the on-disk shape ────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct EmptyWindowSession {
    version: i64,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "creationDate")]
    creation_date: i64,
    #[serde(rename = "customTitle")]
    #[allow(dead_code)]
    custom_title: String,
    requests: Vec<EmptyWindowReq>,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowReq {
    message: EmptyWindowMsg,
    response: Vec<EmptyWindowResp>,
    result: Option<EmptyWindowResult>,
    agent: Option<EmptyWindowAgent>,
    #[serde(rename = "isCanceled")]
    is_canceled: bool,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowMsg {
    parts: Vec<EmptyWindowPart>,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowPart {
    text: String,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowResp {
    value: String,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowResult {
    timings: Option<EmptyWindowTimings>,
    #[allow(dead_code)]
    metadata: Value,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowTimings {
    #[serde(rename = "firstProgress")]
    first_progress: i64,
    #[serde(rename = "totalElapsed")]
    total_elapsed: i64,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
struct EmptyWindowAgent {
    id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct MutationEntry {
    kind: i64,
    k: Vec<String>,
    v: Value,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct EmptyWindowSnapshot {
    #[allow(dead_code)]
    version: i64,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "creationDate")]
    creation_date: i64,
    requests: Vec<EmptyWindowReq>,
    #[serde(rename = "inputState")]
    input_state: Option<InputState>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct InputState {
    mode: Option<ModeInfo>,
    #[serde(rename = "selectedModel")]
    selected_model: Option<SelectedModelInfo>,
    #[serde(rename = "permissionLevel")]
    #[allow(dead_code)]
    perm_level: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ModeInfo {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    kind: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SelectedModelInfo {
    identifier: String,
    metadata: Option<ModelMetadata>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ModelMetadata {
    #[allow(dead_code)]
    id: String,
    name: String,
    #[allow(dead_code)]
    vendor: String,
}

// ─── Entry point ─────────────────────────────────────────────────────────────

pub fn process_empty_window_sessions(user_id: &str, state: &FileState, dedup: &Deduplicator) {
    let global_base = global_storage_base();
    let dir = global_base.join("emptyWindowChatSessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".json") {
            process_json_session(&path, user_id, state, dedup);
        } else if name_str.ends_with(".jsonl") {
            process_jsonl_session(&path, user_id, state, dedup);
        }
    }
}

fn process_json_session(
    file_path: &PathBuf,
    user_id: &str,
    state: &FileState,
    dedup: &Deduplicator,
) {
    let Ok(data) = std::fs::read(file_path) else {
        return;
    };
    let key = file_path.to_string_lossy().into_owned();
    let prev_size = state.get(&key);
    if (data.len() as i64) <= prev_size {
        return;
    }
    state.set(&key, data.len() as i64);

    let Ok(session) = serde_json::from_slice::<EmptyWindowSession>(&data) else {
        return;
    };
    if session.version < 3 || session.requests.is_empty() {
        return;
    }
    save_empty_window_exchanges(
        &session.session_id,
        session.creation_date,
        &session.requests,
        "",
        user_id,
        dedup,
    );
}

fn process_jsonl_session(
    file_path: &PathBuf,
    user_id: &str,
    state: &FileState,
    dedup: &Deduplicator,
) {
    let Ok(data) = std::fs::read(file_path) else {
        return;
    };
    let key = file_path.to_string_lossy().into_owned();
    let prev_size = state.get(&key);
    if (data.len() as i64) <= prev_size {
        return;
    }
    state.set(&key, data.len() as i64);

    let text = String::from_utf8_lossy(&data);
    let mut tree: Value = Value::Null;

    for line in text.trim().split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<MutationEntry>(line) else {
            continue;
        };
        match entry.kind {
            0 => tree = entry.v,
            1 => {
                if !tree.is_null() && !entry.k.is_empty() {
                    tree = apply_mutation(tree, &entry.k, entry.v);
                }
            }
            _ => {}
        }
    }

    if tree.is_null() {
        return;
    }
    let Ok(snap) = serde_json::from_value::<EmptyWindowSnapshot>(tree) else {
        return;
    };

    let mut model_name = String::new();
    if let Some(state) = &snap.input_state {
        if let Some(sel) = &state.selected_model {
            model_name = match &sel.metadata {
                Some(m) => m.name.clone(),
                None => sel.identifier.clone(),
            };
        }
    }

    if snap.session_id.is_empty() || snap.requests.is_empty() {
        return;
    }
    save_empty_window_exchanges(
        &snap.session_id,
        snap.creation_date,
        &snap.requests,
        &model_name,
        user_id,
        dedup,
    );
}

fn apply_mutation(root: Value, keys: &[String], raw: Value) -> Value {
    if keys.is_empty() {
        return raw;
    }
    let key = &keys[0];
    let rest = &keys[1..];

    if let Ok(idx) = key.parse::<usize>() {
        let mut arr = match root {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        while arr.len() <= idx {
            arr.push(Value::Null);
        }
        let current = arr[idx].take();
        arr[idx] = apply_mutation(current, rest, raw);
        return Value::Array(arr);
    }

    let mut m = match root {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let current = m.remove(key).unwrap_or(Value::Null);
    let new_value = apply_mutation(current, rest, raw);
    m.insert(key.clone(), new_value);
    Value::Object(m)
}

fn save_empty_window_exchanges(
    session_id: &str,
    creation_date_ms: i64,
    requests: &[EmptyWindowReq],
    model: &str,
    user_id: &str,
    dedup: &Deduplicator,
) {
    let created_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(creation_date_ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();

    let mut new_count = 0usize;
    for (i, req) in requests.iter().enumerate() {
        if req.is_canceled {
            continue;
        }
        let prompt_text = extract_empty_window_prompt(req);
        if prompt_text.trim().is_empty() {
            continue;
        }
        let response_text = extract_empty_window_response(req);

        let captured_at = if creation_date_ms > 0 {
            let offset = creation_date_ms + (i as i64) * 30_000;
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(offset)
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                .unwrap_or_else(|| created_at.clone())
        } else {
            created_at.clone()
        };

        let hash = supabase::prompt_content_hash_v2(session_id, &prompt_text, &captured_at);
        if dedup.is_duplicate(session_id, &hash) {
            continue;
        }
        if is_draft_saved_at(session_id, &prompt_text, &captured_at) {
            dedup.mark(session_id, &hash);
            continue;
        }

        let mut file_context = serde_json::Map::new();
        file_context.insert(
            "capture_schema".into(),
            Value::Number(versions::CAPTURE_SCHEMA_VERSION.into()),
        );
        let is_agentic = req.agent.as_ref().is_some_and(|a| !a.id.is_empty());
        file_context.insert("is_agentic".into(), Value::Bool(is_agentic));
        if let Some(result) = &req.result {
            if let Some(timings) = &result.timings {
                file_context.insert(
                    "response_duration_ms".into(),
                    Value::Number(timings.total_elapsed.into()),
                );
                if timings.first_progress > 0 {
                    file_context.insert(
                        "first_response_ms".into(),
                        Value::Number(timings.first_progress.into()),
                    );
                }
            }
        }

        let record = PromptRecord {
            id: supabase::prompt_id_v2(session_id, &prompt_text, &captured_at),
            content_hash: hash.clone(),
            session_id: session_id.to_string(),
            prompt_text,
            response_text,
            model: model.to_string(),
            source: "vscode".into(),
            capture_method: "file-watcher".into(),
            captured_at,
            user_id: user_id.to_string(),
            file_context: Some(file_context),
            ..Default::default()
        };

        if let Err(e) = store::save_draft(&record, &[], "", "") {
            display::print_error("vscode", &format!("Failed to save empty-window draft: {e}"));
            continue;
        }
        dedup.mark(session_id, &hash);
        new_count += 1;
    }

    if new_count > 0 {
        let last = requests
            .last()
            .map(extract_empty_window_prompt)
            .unwrap_or_default();
        display::print_drafted(&display::DraftDisplayOptions {
            project_name: "(no workspace)",
            prompt_text: &last,
            exchange_count: new_count as u64,
            ..Default::default()
        });
    }
}

fn extract_empty_window_prompt(req: &EmptyWindowReq) -> String {
    req.message
        .parts
        .iter()
        .filter(|p| !p.text.is_empty())
        .map(|p| p.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_empty_window_response(req: &EmptyWindowReq) -> String {
    req.response
        .iter()
        .filter(|r| !r.value.is_empty())
        .map(|r| r.value.clone())
        .collect::<Vec<_>>()
        .join("\n")
}
