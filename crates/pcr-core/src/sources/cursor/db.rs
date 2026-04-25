//! Reader for Cursor's internal `state.vscdb` SQLite database. Direct port
//! of `cli/internal/sources/cursor/db.go`.
//!
//! Cursor stores composer (session) data in a key-value table
//! `cursorDiskKV` with keys like `composerData:<sessionId>` and
//! `bubbleId:<composerId>:<bubbleId>`. We open it read-only and extract
//! the metadata the watcher needs, cached for 60 s to avoid hammering
//! SQLite.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct BubbleMeta {
    /// 1 = user, 2 = assistant.
    pub ty: i64,
    pub bubble_id: String,
    pub text: String,
    /// ISO8601 from v14+.
    pub created_at: String,
    /// Set on the final assistant bubble when the turn completes.
    pub turn_duration_ms: Option<i64>,
    pub is_agentic: Option<bool>,
    pub relevant_files: Vec<String>,
    pub unified_mode: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionMeta {
    pub bubbles: Vec<BubbleMeta>,
    pub model_name: String,
    pub is_agentic: bool,
    pub unified_mode: String,
    pub composer_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionData {
    pub session_id: String,
    pub schema_v: i32,
    pub name: String,
    pub subtitle: String,
    pub model_name: String,
    pub is_agentic: bool,
    pub unified_mode: String,
    pub plan_mode_used: Option<bool>,
    pub debug_mode_used: Option<bool>,
    pub branch: String,
    pub context_tokens_used: Option<i64>,
    pub context_token_limit: Option<i64>,
    pub files_changed_count: Option<i64>,
    pub total_lines_added: Option<i64>,
    pub total_lines_removed: Option<i64>,
    pub session_created_at: Option<i64>,
    pub session_updated_at: Option<i64>,
    pub meta: serde_json::Map<String, Value>,
}

// ─── DB singleton ────────────────────────────────────────────────────────────

pub fn cursor_db_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    #[cfg(target_os = "macos")]
    {
        return home
            .join("Library")
            .join("Application Support")
            .join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData").join("Roaming"));
        return base
            .join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home.join(".config")
            .join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
    }
}

static DB: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

fn open_cursor_db() -> Option<std::sync::MutexGuard<'static, Option<Connection>>> {
    let mutex = DB.get_or_init(|| {
        let path = cursor_db_path();
        if !path.exists() {
            return Mutex::new(None);
        }
        // Read-only + immutable so we don't contend with Cursor's writer.
        let uri = format!("file:{}?mode=ro&immutable=1", path.display());
        match Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(conn) => Mutex::new(Some(conn)),
            Err(_) => Mutex::new(None),
        }
    });
    let guard = mutex.lock().ok()?;
    if guard.is_none() {
        return None;
    }
    Some(guard)
}

// ─── Metadata cache ──────────────────────────────────────────────────────────

struct CacheEntry {
    meta: Option<SessionMeta>,
    ts: Instant,
}

static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const CACHE_TTL: Duration = Duration::from_secs(60);

pub fn invalidate_session_cache(session_id: &str) {
    if let Ok(mut guard) = cache().lock() {
        guard.remove(session_id);
    }
}

/// One-shot per-session warning when Cursor writes a row without a
/// `composerId` field. Without this dedupe the verbose event log would
/// flood every cache-miss (~once per minute per affected session).
static WARNED_MISSING_COMPOSER: OnceLock<Mutex<std::collections::HashSet<String>>> =
    OnceLock::new();

fn warn_missing_composer_id_once(session_id: &str) {
    let mutex =
        WARNED_MISSING_COMPOSER.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let Ok(mut guard) = mutex.lock() else {
        return;
    };
    if !guard.insert(session_id.to_string()) {
        return;
    }
    drop(guard);
    let short: String = session_id.chars().take(8).collect();
    crate::display::print_verbose_event(
        "cursor",
        &format!("[{short}] composerId missing in cursorDiskKV — falling back to sessionId"),
    );
}

/// `cli/internal/sources/cursor/db.go::GetSessionMeta`.
pub fn get_session_meta(session_id: &str) -> Option<SessionMeta> {
    if let Ok(guard) = cache().lock() {
        if let Some(e) = guard.get(session_id) {
            if e.ts.elapsed() < CACHE_TTL {
                return e.meta.clone();
            }
        }
    }

    let guard_opt = open_cursor_db();
    let Some(guard) = guard_opt else {
        store_meta_cache(session_id, None);
        return None;
    };
    let db = guard.as_ref().expect("DB guard invariant");

    let composer_key = format!("composerData:{session_id}");
    let row = db
        .query_row(
            r#"SELECT
                 json_extract(value, '$.isAgentic'),
                 json_extract(value, '$.unifiedMode'),
                 json_extract(value, '$.modelConfig'),
                 json_extract(value, '$.fullConversationHeadersOnly')
               FROM cursorDiskKV
               WHERE key = ?"#,
            [&composer_key],
            |r| {
                let is_agentic: Option<i64> = r.get(0)?;
                let unified_mode: Option<String> = r.get(1)?;
                let model_config: Option<String> = r.get(2)?;
                let headers_only: Option<String> = r.get(3)?;
                Ok((is_agentic, unified_mode, model_config, headers_only))
            },
        )
        .optional();
    let Ok(Some((is_agentic, unified_mode, model_config, headers_only))) = row else {
        drop(guard);
        store_meta_cache(session_id, None);
        return None;
    };

    let agentic = is_agentic == Some(1);
    let mut model_name = String::new();
    if let Some(s) = &model_config {
        if let Ok(mc) = serde_json::from_str::<Value>(s) {
            if let Some(n) = mc.get("modelName").and_then(|v| v.as_str()) {
                model_name = n.to_string();
            }
        }
    }
    let um = unified_mode.unwrap_or_default();

    let composer_id_raw: String = db
        .query_row(
            "SELECT json_extract(value, '$.composerId') FROM cursorDiskKV WHERE key = ?",
            [&composer_key],
            |r| r.get::<_, Option<String>>(0).map(|v| v.unwrap_or_default()),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or_default();
    // Cursor sometimes writes `composerId` equal to the session id, and
    // some sessions omit the field entirely (older schemas, corrupted
    // rows). Fall back to the session id when missing — the bubble keys
    // are `bubbleId:<composerId>:<bubbleId>`, so without a composer id
    // every bubble lookup produces NULL and the entire session silently
    // disappears. The first time we hit this for a given session id we
    // emit a verbose event so `--verbose` users see what happened.
    let composer_id = if composer_id_raw.is_empty() {
        warn_missing_composer_id_once(session_id);
        session_id.to_string()
    } else {
        composer_id_raw
    };

    let mut bubbles: Vec<BubbleMeta> = Vec::new();
    if let Some(raw) = &headers_only {
        if let Ok(headers) = serde_json::from_str::<Vec<HeaderRow>>(raw) {
            for h in headers {
                let mut b = BubbleMeta {
                    ty: h.ty,
                    bubble_id: h.bubble_id.clone(),
                    ..Default::default()
                };
                if !composer_id.is_empty() && !h.bubble_id.is_empty() {
                    let bkey = format!("bubbleId:{composer_id}:{}", h.bubble_id);
                    let bval: Option<String> = db
                        .query_row(
                            "SELECT value FROM cursorDiskKV WHERE key = ?",
                            [&bkey],
                            |r| r.get(0),
                        )
                        .optional()
                        .ok()
                        .flatten();
                    if let Some(s) = bval {
                        if let Ok(Value::Object(bd)) = serde_json::from_str::<Value>(&s) {
                            b.text = bd
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            b.created_at = bd
                                .get("createdAt")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            b.unified_mode = bd
                                .get("unifiedMode")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some(arr) = bd.get("relevantFiles").and_then(|v| v.as_array()) {
                                b.relevant_files = arr
                                    .iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect();
                            }
                            if let Some(ag) = bd.get("isAgentic").and_then(|v| v.as_bool()) {
                                b.is_agentic = Some(ag);
                            }
                            if let Some(dur) = bd.get("turnDurationMs").and_then(|v| v.as_f64()) {
                                b.turn_duration_ms = Some(dur as i64);
                            }
                        }
                    }
                }
                bubbles.push(b);
            }
        }
    }

    let meta = SessionMeta {
        bubbles,
        model_name,
        is_agentic: agentic,
        unified_mode: um,
        composer_id,
    };
    drop(guard);
    let out = Some(meta.clone());
    store_meta_cache(session_id, out.clone());
    out
}

#[derive(Debug, serde::Deserialize)]
struct HeaderRow {
    #[serde(rename = "bubbleId", default)]
    bubble_id: String,
    #[serde(rename = "type", default)]
    ty: i64,
}

fn store_meta_cache(session_id: &str, meta: Option<SessionMeta>) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(
            session_id.to_string(),
            CacheEntry {
                meta,
                ts: Instant::now(),
            },
        );
    }
}

// ─── Full session data ───────────────────────────────────────────────────────

static STRIPPED_FIELDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
static STRUCTURED_FIELDS: OnceLock<HashSet<&'static str>> = OnceLock::new();

fn stripped_fields() -> &'static HashSet<&'static str> {
    STRIPPED_FIELDS.get_or_init(|| {
        [
            "fullConversationHeadersOnly",
            "conversationMap",
            "conversationState",
            "blobEncryptionKey",
            "speculativeSummarizationEncryptionKey",
            "richText",
            "generatingBubbleIds",
            "codeBlockData",
            "originalFileStates",
        ]
        .into_iter()
        .collect()
    })
}

fn structured_fields() -> &'static HashSet<&'static str> {
    STRUCTURED_FIELDS.get_or_init(|| {
        [
            "_v",
            "composerId",
            "isAgentic",
            "unifiedMode",
            "forceMode",
            "modelConfig",
            "name",
            "subtitle",
            "planModeSuggestionUsed",
            "debugModeSuggestionUsed",
            "contextTokensUsed",
            "contextTokenLimit",
            "filesChangedCount",
            "totalLinesAdded",
            "totalLinesRemoved",
            "activeBranch",
            "createdOnBranch",
            "createdAt",
            "lastUpdatedAt",
        ]
        .into_iter()
        .collect()
    })
}

pub fn get_full_session_data(session_id: &str) -> Option<SessionData> {
    let guard = open_cursor_db()?;
    let db = guard.as_ref()?;
    let composer_key = format!("composerData:{session_id}");
    let raw: String = db
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?",
            [&composer_key],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    drop(guard);

    let Value::Object(obj) = serde_json::from_str::<Value>(&raw).ok()? else {
        return None;
    };

    let mut sd = SessionData {
        session_id: session_id.to_string(),
        ..Default::default()
    };
    sd.schema_v = get_f64(&obj, "_v") as i32;
    sd.is_agentic = get_bool(&obj, "isAgentic");
    sd.unified_mode = get_str(&obj, "unifiedMode");
    sd.name = get_str(&obj, "name");
    sd.subtitle = get_str(&obj, "subtitle");

    if let Some(mc) = obj.get("modelConfig").and_then(|v| v.as_object()) {
        sd.model_name = get_str(mc, "modelName");
    }
    if let Some(ab) = obj.get("activeBranch").and_then(|v| v.as_object()) {
        sd.branch = get_str(ab, "branchName");
    }
    sd.plan_mode_used = get_int(&obj, "planModeSuggestionUsed").map(|v| v == 1);
    sd.debug_mode_used = get_int(&obj, "debugModeSuggestionUsed").map(|v| v == 1);
    sd.context_tokens_used = get_int(&obj, "contextTokensUsed");
    sd.context_token_limit = get_int(&obj, "contextTokenLimit");
    sd.files_changed_count = get_int(&obj, "filesChangedCount");
    sd.total_lines_added = get_int(&obj, "totalLinesAdded");
    sd.total_lines_removed = get_int(&obj, "totalLinesRemoved");
    sd.session_created_at = get_int(&obj, "createdAt");
    sd.session_updated_at = get_int(&obj, "lastUpdatedAt");

    let stripped = stripped_fields();
    let structured = structured_fields();
    let mut meta = serde_json::Map::new();
    for (k, v) in &obj {
        if !structured.contains(k.as_str()) && !stripped.contains(k.as_str()) && !v.is_null() {
            meta.insert(k.clone(), v.clone());
        }
    }
    sd.meta = meta;
    Some(sd)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn get_f64(m: &serde_json::Map<String, Value>, k: &str) -> f64 {
    m.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn get_str(m: &serde_json::Map<String, Value>, k: &str) -> String {
    m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn get_bool(m: &serde_json::Map<String, Value>, k: &str) -> bool {
    match m.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() == Some(1.0),
        _ => false,
    }
}

fn get_int(m: &serde_json::Map<String, Value>, k: &str) -> Option<i64> {
    m.get(k).and_then(|v| v.as_f64()).map(|n| n as i64)
}

// ─── Session state poller (used by session_state_watcher) ───────────────────

pub struct ComposerStateRow {
    pub composer_id: String,
    pub unified_mode: String,
    pub model_name: String,
    pub context_tokens_used: i64,
    pub context_token_limit: i64,
}

/// Query `composerData:%` rows ordered by `lastUpdatedAt DESC LIMIT 50`.
pub fn all_composer_state_rows() -> Vec<ComposerStateRow> {
    let Some(guard) = open_cursor_db() else {
        return Vec::new();
    };
    let db = guard.as_ref().expect("DB guard invariant");
    let mut stmt = match db.prepare(
        r#"SELECT
             json_extract(value, '$.composerId'),
             json_extract(value, '$.unifiedMode'),
             json_extract(value, '$.modelConfig'),
             json_extract(value, '$.contextTokensUsed'),
             json_extract(value, '$.contextTokenLimit'),
             json_extract(value, '$.lastUpdatedAt')
           FROM cursorDiskKV
           WHERE key LIKE 'composerData:%'
             AND json_extract(value, '$.composerId') IS NOT NULL
             AND json_extract(value, '$.lastUpdatedAt') IS NOT NULL
           ORDER BY json_extract(value, '$.lastUpdatedAt') DESC
           LIMIT 50"#,
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<ComposerStateRow> = Vec::new();
    let rows = stmt.query_map([], |r| {
        let composer_id: String = r.get::<_, Option<String>>(0)?.unwrap_or_default();
        let unified_mode: Option<String> = r.get(1)?;
        let model_config: Option<String> = r.get(2)?;
        let ctx_used: Option<f64> = r.get(3)?;
        let ctx_limit: Option<f64> = r.get(4)?;
        Ok((composer_id, unified_mode, model_config, ctx_used, ctx_limit))
    });
    let Ok(mut rows) = rows else { return out };
    while let Some(Ok((composer_id, unified_mode, model_config, ctx_used, ctx_limit))) = rows.next()
    {
        if composer_id.is_empty() {
            continue;
        }
        let mut model_name = String::new();
        if let Some(raw) = &model_config {
            if let Ok(mc) = serde_json::from_str::<Value>(raw) {
                if let Some(n) = mc.get("modelName").and_then(|v| v.as_str()) {
                    model_name = n.to_string();
                }
            }
        }
        out.push(ComposerStateRow {
            composer_id,
            unified_mode: unified_mode.unwrap_or_default(),
            model_name,
            context_tokens_used: ctx_used.unwrap_or(0.0) as i64,
            context_token_limit: ctx_limit.unwrap_or(0.0) as i64,
        });
    }
    out
}
