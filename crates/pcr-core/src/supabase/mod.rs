//! Supabase RPC client. Mirrors `cli/internal/supabase/supabase.go`.
//!
//! Hashing algorithms, HTTP request shapes, and header usage are identical
//! to the Go version so a Rust-built CLI writes rows that deduplicate
//! correctly against rows written by previous Go builds.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch_name: String,
    pub prompt_text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub source: String,
    pub capture_method: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_context: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub captured_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub team_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
}

#[derive(Debug, Clone, Default)]
pub struct CursorSessionData {
    pub session_id: String,
    pub project_name: String,
    pub branch: String,
    pub model_name: String,
    pub is_agentic: Option<bool>,
    pub unified_mode: Option<bool>,
    pub plan_mode_used: Option<bool>,
    pub debug_mode_used: Option<bool>,
    pub schema_v: i32,
    pub context_tokens_used: Option<i64>,
    pub context_token_limit: Option<i64>,
    pub files_changed_count: Option<i64>,
    pub total_lines_added: Option<i64>,
    pub total_lines_removed: Option<i64>,
    pub session_created_at: Option<i64>,
    pub session_updated_at: Option<i64>,
    pub commit_sha_start: String,
    pub commit_sha_end: String,
    pub commit_shas: Vec<String>,
    pub meta: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TouchedProject {
    pub project_id: String,
    pub branch: String,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Default)]
pub struct BundleData {
    pub bundle_id: String,
    pub message: String,
    pub source: String,
    pub project_name: String,
    pub session_shas: Vec<String>,
    pub head_sha: String,
    pub exchange_count: i64,
    pub committed_at: String,
    pub touched_projects: Vec<TouchedProject>,
}

// ─── Hashing ─────────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

fn as_uuid_fmt(hex: &str) -> String {
    // 8-4-4-4-12 formatting of a 32-hex-char prefix. Matches the Go
    // `fmt.Sprintf("%s-%s-%s-%s-%s", hex[:8], hex[8:12], hex[12:16], hex[16:20], hex[20:32])`.
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32],
    )
}

/// SHA-256 hex digest of `session_id + \x00 + prompt_text + \x00 + response_text`.
pub fn prompt_content_hash(session_id: &str, prompt_text: &str, response_text: &str) -> String {
    sha256_hex(&format!("{session_id}\x00{prompt_text}\x00{response_text}"))
}

/// V2 hash: includes `captured_at` instead of the response so two identical
/// prompts sent at different times in the same session get distinct IDs.
pub fn prompt_content_hash_v2(session_id: &str, prompt_text: &str, captured_at: &str) -> String {
    sha256_hex(&format!("{session_id}\x00{prompt_text}\x00{captured_at}"))
}

pub fn prompt_id(session_id: &str, prompt_text: &str, response_text: &str) -> String {
    as_uuid_fmt(&prompt_content_hash(session_id, prompt_text, response_text))
}

pub fn prompt_id_v2(session_id: &str, prompt_text: &str, captured_at: &str) -> String {
    as_uuid_fmt(&prompt_content_hash_v2(
        session_id,
        prompt_text,
        captured_at,
    ))
}

// ─── HTTP RPC plumbing ───────────────────────────────────────────────────────

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(format!("pcr-cli/{}", crate::VERSION))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client build")
}

fn rpc(token: &str, function_name: &str, payload: &Value) -> Result<Vec<u8>> {
    let url = format!("{}/rest/v1/rpc/{}", config::SUPABASE_URL, function_name);
    let mut req = client()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("apikey", config::SUPABASE_KEY)
        .body(serde_json::to_vec(payload)?);
    if !token.is_empty() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send()?;
    let status = resp.status();
    let body = resp.bytes()?;
    if !status.is_success() {
        return Err(anyhow!(
            "supabase rpc {function_name}: {} — {}",
            status,
            String::from_utf8_lossy(&body)
        ));
    }
    Ok(body.to_vec())
}

// ─── Public RPC calls ────────────────────────────────────────────────────────

/// Batch-upsert prompts. Returns count inserted/updated.
pub fn upsert_prompts(token: &str, records: &[PromptRecord]) -> Result<i64> {
    if records.is_empty() {
        return Ok(0);
    }
    let enriched: Vec<PromptRecord> = records
        .iter()
        .cloned()
        .map(|mut r| {
            r.id = String::new();
            if r.content_hash.is_empty() {
                r.content_hash =
                    prompt_content_hash(&r.session_id, &r.prompt_text, &r.response_text);
            }
            r
        })
        .collect();
    let data = rpc(token, "upsert_prompts", &json!({ "p_records": enriched }))?;
    let count: i64 = serde_json::from_slice(&data).unwrap_or(0);
    Ok(count)
}

pub fn upsert_prompt(token: &str, record: PromptRecord) -> Result<bool> {
    let n = upsert_prompts(token, &[record])?;
    Ok(n > 0)
}

/// Validate a CLI paste-token. Returns the userId or an error.
pub fn validate_cli_token(token: &str) -> Result<String> {
    let data = rpc("", "validate_cli_token", &json!({ "p_token": token }))?;
    let user_id: String = serde_json::from_slice(&data).unwrap_or_default();
    Ok(user_id)
}

pub fn register_project(
    token: &str,
    name: &str,
    git_remote: &str,
    local_path: &str,
    user_id: &str,
) -> Result<String> {
    let payload = json!({
        "p_name": name,
        "p_git_remote": git_remote,
        "p_local_path": local_path,
        "p_user_id": nullable(user_id),
    });
    let data = rpc(token, "register_project", &payload)?;
    let project_id: String = serde_json::from_slice(&data).unwrap_or_default();
    Ok(project_id)
}

pub fn upsert_cursor_session(
    token: &str,
    data: &CursorSessionData,
    project_id: &str,
    user_id: &str,
) -> Result<()> {
    let mut payload = serde_json::Map::new();
    payload.insert("session_id".into(), Value::String(data.session_id.clone()));
    payload.insert("project_id".into(), nullable(project_id));
    payload.insert("user_id".into(), nullable(user_id));
    payload.insert("model_name".into(), nullable(&data.model_name));
    payload.insert("branch".into(), nullable(&data.branch));
    payload.insert(
        "is_agentic".into(),
        data.is_agentic.map(Value::Bool).unwrap_or(Value::Null),
    );
    payload.insert(
        "unified_mode".into(),
        data.unified_mode.map(Value::Bool).unwrap_or(Value::Null),
    );
    payload.insert(
        "plan_mode_used".into(),
        data.plan_mode_used.map(Value::Bool).unwrap_or(Value::Null),
    );
    payload.insert(
        "debug_mode_used".into(),
        data.debug_mode_used.map(Value::Bool).unwrap_or(Value::Null),
    );
    payload.insert("cursor_schema_v".into(), json!(data.schema_v));
    payload.insert(
        "context_tokens_used".into(),
        data.context_tokens_used
            .map(|n| json!(n))
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "context_token_limit".into(),
        data.context_token_limit
            .map(|n| json!(n))
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "files_changed_count".into(),
        data.files_changed_count
            .map(|n| json!(n))
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "total_lines_added".into(),
        data.total_lines_added
            .map(|n| json!(n))
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "total_lines_removed".into(),
        data.total_lines_removed
            .map(|n| json!(n))
            .unwrap_or(Value::Null),
    );
    payload.insert("commit_sha_start".into(), nullable(&data.commit_sha_start));
    payload.insert("commit_sha_end".into(), nullable(&data.commit_sha_end));
    payload.insert("commit_shas".into(), json!(data.commit_shas));
    payload.insert("meta".into(), Value::Object(data.meta.clone()));
    if let Some(ts) = data.session_created_at {
        payload.insert("session_created_at".into(), json!(ts));
    }
    if let Some(ts) = data.session_updated_at {
        payload.insert("session_updated_at".into(), json!(ts));
    }

    rpc(
        token,
        "upsert_cursor_session",
        &json!({ "p_session": Value::Object(payload) }),
    )?;
    Ok(())
}

pub fn upsert_bundle(token: &str, data: &BundleData, user_id: &str) -> Result<String> {
    let touched = if data.touched_projects.is_empty() {
        Value::Array(Vec::new())
    } else {
        serde_json::to_value(&data.touched_projects)?
    };
    let shas = Value::Array(
        data.session_shas
            .iter()
            .map(|s| Value::String(s.clone()))
            .collect(),
    );
    let bundle = json!({
        "bundle_id":        data.bundle_id,
        "message":          data.message,
        "source":           data.source,
        "project_name":     nullable(&data.project_name),
        "session_shas":     shas,
        "head_sha":         nullable(&data.head_sha),
        "exchange_count":   data.exchange_count,
        "committed_at":     nullable(&data.committed_at),
        "touched_projects": touched,
    });
    let resp = rpc(
        token,
        "upsert_bundle",
        &json!({ "p_bundle": bundle, "p_user_id": nullable(user_id) }),
    )?;
    let remote_id: String = serde_json::from_slice(&resp).unwrap_or_default();
    Ok(remote_id)
}

pub fn upsert_bundle_prompts(
    token: &str,
    items: &[Value],
    diffs: &[Value],
    user_id: &str,
) -> Result<()> {
    if !items.is_empty() {
        rpc(
            token,
            "upsert_prompts",
            &json!({ "p_records": items, "p_user_id": nullable(user_id) }),
        )?;
    }
    if !diffs.is_empty() {
        rpc(token, "upsert_git_diffs", &json!({ "p_diffs": diffs }))?;
    }
    Ok(())
}

pub fn pull_bundle(token: &str, remote_id: &str) -> Result<Value> {
    let url = format!(
        "{}/rest/v1/bundles?bundle_id=eq.{}&select=*&limit=1",
        config::SUPABASE_URL,
        urlencoding::encode(remote_id),
    );
    let mut req = client()
        .get(&url)
        .header("apikey", config::SUPABASE_KEY)
        .header("Accept", "application/json");
    if !token.is_empty() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send()?;
    let status = resp.status();
    let body = resp.bytes()?;
    if !status.is_success() {
        return Err(anyhow!(
            "pull bundle: {} — {}",
            status,
            String::from_utf8_lossy(&body)
        ));
    }
    let rows: Vec<Value> = serde_json::from_slice(&body)
        .map_err(|e| anyhow!("pull bundle: failed to parse response: {e}"))?;
    rows.into_iter()
        .next()
        .ok_or_else(|| anyhow!("bundle \"{}\" not found", remote_id))
}

fn nullable(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_deterministic() {
        let a = prompt_content_hash("sess", "hello", "world");
        let b = prompt_content_hash("sess", "hello", "world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        // Any change in any field must produce a different hash.
        assert_ne!(a, prompt_content_hash("sess2", "hello", "world"));
        assert_ne!(a, prompt_content_hash("sess", "hello2", "world"));
        assert_ne!(a, prompt_content_hash("sess", "hello", "world2"));
    }

    #[test]
    fn v2_hash_differs_from_v1_with_timestamp() {
        let v1 = prompt_content_hash("s", "p", "r");
        let v2 = prompt_content_hash_v2("s", "p", "2026-04-24T10:00:00Z");
        assert_ne!(v1, v2);
        // V2 only depends on captured_at, not response_text.
        assert_eq!(
            prompt_content_hash_v2("s", "p", "2026-04-24T10:00:00Z"),
            prompt_content_hash_v2("s", "p", "2026-04-24T10:00:00Z"),
        );
    }

    #[test]
    fn prompt_id_is_uuid_shape() {
        let id = prompt_id("a", "b", "c");
        let hex: Vec<&str> = id.split('-').collect();
        assert_eq!(hex.len(), 5);
        assert_eq!(hex[0].len(), 8);
        assert_eq!(hex[1].len(), 4);
        assert_eq!(hex[2].len(), 4);
        assert_eq!(hex[3].len(), 4);
        assert_eq!(hex[4].len(), 12);
    }

    #[test]
    fn nullable_helper_null_on_empty() {
        assert_eq!(nullable(""), Value::Null);
        assert_eq!(nullable("x"), Value::String("x".into()));
    }
}
