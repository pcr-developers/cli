//! Draft CRUD. Mirrors `cli/internal/store/drafts.go`.
//!
//! The row shape, query semantics, and dedup rules (content-hash-based) are
//! identical to the Go implementation so a user's existing local DB keeps
//! working across the upgrade.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::db::{null_if_empty, open};
use crate::supabase::{self, PromptRecord};
use crate::util::time::now_rfc3339;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftStatus {
    Draft,
    Staged,
    Committed,
    Pushed,
}

impl DraftStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DraftStatus::Draft => "draft",
            DraftStatus::Staged => "staged",
            DraftStatus::Committed => "committed",
            DraftStatus::Pushed => "pushed",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DraftRecord {
    pub id: String,
    pub content_hash: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
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
    pub captured_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_commit_shas: Vec<String>,
    pub status: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub git_diff: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub head_sha: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
}

impl DraftRecord {
    /// Mirrors `DraftRecord.TouchedProjectIDs` — returns every project ID
    /// recorded in `file_context.touched_project_ids`.
    pub fn touched_project_ids(&self) -> Vec<String> {
        let Some(fc) = &self.file_context else {
            return Vec::new();
        };
        let Some(raw) = fc.get("touched_project_ids") else {
            return Vec::new();
        };
        if let Some(arr) = raw.as_array() {
            return arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
        Vec::new()
    }
}

/// Insert/upsert a draft. Mirrors `SaveDraft`.
pub fn save_draft(
    record: &PromptRecord,
    session_shas: &[String],
    git_diff: &str,
    head_sha: &str,
) -> Result<()> {
    let conn = open();

    let id = if record.id.is_empty() {
        supabase::prompt_id(&record.session_id, &record.prompt_text, "")
    } else {
        record.id.clone()
    };
    let hash = if record.content_hash.is_empty() {
        supabase::prompt_content_hash(&record.session_id, &record.prompt_text, "")
    } else {
        record.content_hash.clone()
    };

    let tool_calls_json = if record.tool_calls.is_empty() {
        None
    } else {
        serde_json::to_string(&record.tool_calls).ok()
    };
    let file_context_json = record
        .file_context
        .as_ref()
        .filter(|m| !m.is_empty())
        .and_then(|m| serde_json::to_string(m).ok());
    let session_shas_json = if session_shas.is_empty() {
        None
    } else {
        serde_json::to_string(session_shas).ok()
    };

    let captured_at = if record.captured_at.is_empty() {
        now_rfc3339()
    } else {
        record.captured_at.clone()
    };

    conn.execute(
        r#"INSERT INTO drafts (
            id, content_hash, session_id, project_id, project_name, branch_name,
            prompt_text, response_text, model, source, capture_method,
            tool_calls, file_context, captured_at, session_commit_shas, status, git_diff, head_sha,
            permission_mode
          ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'draft', ?, ?, ?)
          ON CONFLICT(content_hash) DO UPDATE SET
            response_text  = COALESCE(excluded.response_text, drafts.response_text),
            tool_calls     = COALESCE(excluded.tool_calls,    drafts.tool_calls),
            file_context   = COALESCE(excluded.file_context,  drafts.file_context),
            model          = COALESCE(excluded.model,          drafts.model),
            git_diff       = COALESCE(excluded.git_diff,       drafts.git_diff),
            head_sha       = COALESCE(excluded.head_sha,       drafts.head_sha),
            project_id     = COALESCE(NULLIF(drafts.project_id,   ''), excluded.project_id),
            project_name   = COALESCE(NULLIF(drafts.project_name, ''), excluded.project_name),
            permission_mode = COALESCE(excluded.permission_mode, drafts.permission_mode)
          WHERE drafts.status = 'draft'"#,
        params![
            id,
            hash,
            record.session_id,
            null_if_empty(&record.project_id),
            record.project_name,
            null_if_empty(&record.branch_name),
            record.prompt_text,
            null_if_empty(&record.response_text),
            null_if_empty(&record.model),
            record.source,
            record.capture_method,
            tool_calls_json,
            file_context_json,
            captured_at,
            session_shas_json,
            null_if_empty(git_diff),
            null_if_empty(head_sha),
            null_if_empty(&record.permission_mode),
        ],
    )?;
    Ok(())
}

pub fn is_draft_saved_by_bubble(session_id: &str, bubble_id: &str) -> bool {
    let conn = open();
    conn.query_row(
        "SELECT 1 FROM saved_bubbles WHERE session_id = ? AND bubble_id = ?",
        params![session_id, bubble_id],
        |_| Ok(()),
    )
    .optional()
    .unwrap_or(None)
    .is_some()
}

pub fn mark_bubble_saved(session_id: &str, bubble_id: &str, draft_hash: &str) -> Result<()> {
    let conn = open();
    conn.execute(
        "INSERT OR IGNORE INTO saved_bubbles (session_id, bubble_id, draft_hash) VALUES (?, ?, ?)",
        params![session_id, bubble_id, draft_hash],
    )?;
    Ok(())
}

/// Lookup: does a draft exist for this (session, prompt) combo? Matches
/// `IsDraftSaved`/`IsDraftSavedAt`.
pub fn is_draft_saved_at(session_id: &str, prompt_text: &str, captured_at: &str) -> bool {
    let conn = open();
    let legacy = supabase::prompt_content_hash(session_id, prompt_text, "");
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM drafts WHERE content_hash = ?",
            params![legacy],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    if exists.is_some() {
        return true;
    }
    if !captured_at.is_empty() {
        let v2 = supabase::prompt_content_hash_v2(session_id, prompt_text, captured_at);
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM drafts WHERE content_hash = ?",
                params![v2],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .unwrap_or(None);
        return exists.is_some();
    }
    false
}

pub fn is_draft_saved(session_id: &str, prompt_text: &str) -> bool {
    is_draft_saved_at(session_id, prompt_text, "")
}

/// Mirrors `UpsertDraftProject`. Merges touched_project_ids and fills
/// primary project_id/name only when empty.
pub fn upsert_draft_project(
    content_hash: &str,
    project_id: &str,
    project_name: &str,
    all_ids: &[String],
) -> Result<()> {
    if project_id.is_empty() && all_ids.is_empty() {
        return Ok(());
    }
    let conn = open();
    let fc_json: Option<String> = conn
        .query_row(
            "SELECT file_context FROM drafts WHERE content_hash = ? AND status = 'draft'",
            params![content_hash],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(None);

    let mut current: serde_json::Map<String, Value> = serde_json::Map::new();
    if let Some(s) = fc_json {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let Value::Object(m) = v {
                current = m;
            }
        }
    }
    if all_ids.len() > 1 {
        current.insert(
            "touched_project_ids".to_string(),
            Value::Array(all_ids.iter().map(|s| Value::String(s.clone())).collect()),
        );
    } else {
        current.remove("touched_project_ids");
    }
    let fc_str = serde_json::to_string(&current).unwrap_or_else(|_| "{}".into());

    conn.execute(
        r#"UPDATE drafts SET
             project_id   = COALESCE(NULLIF(project_id,   ''), ?),
             project_name = COALESCE(NULLIF(project_name, ''), ?),
             file_context = ?
           WHERE content_hash = ? AND status = 'draft'"#,
        params![project_id, project_name, fc_str, content_hash],
    )?;
    Ok(())
}

/// Mirrors `TagUnattributedDrafts`.
pub fn tag_unattributed_drafts(
    primary_id: &str,
    primary_name: &str,
    all_ids: &[String],
) -> Result<()> {
    if primary_id.is_empty() {
        return Ok(());
    }
    let hashes: Vec<String> = {
        let conn = open();
        let mut stmt = conn.prepare(
            "SELECT content_hash FROM drafts WHERE status IN ('draft','staged') AND (project_id IS NULL OR project_id = '')",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for h in hashes {
        upsert_draft_project(&h, primary_id, primary_name, all_ids)?;
    }
    Ok(())
}

/// Mirrors `ClearAllChangedFiles`.
pub fn clear_all_changed_files() -> Result<()> {
    let updates: Vec<(String, String)> = {
        let conn = open();
        let mut stmt = conn.prepare(
            "SELECT content_hash, COALESCE(file_context, '') FROM drafts WHERE status IN ('draft','staged')",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    let conn = open();
    for (hash, fc_raw) in updates {
        let mut fc: serde_json::Map<String, Value> = serde_json::Map::new();
        if !fc_raw.is_empty() {
            if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&fc_raw) {
                fc = m;
            }
        }
        let mut changed = false;
        for key in ["changed_files", "touched_project_ids"] {
            if fc.remove(key).is_some() {
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        let b = serde_json::to_string(&fc).unwrap_or_else(|_| "{}".into());
        conn.execute(
            "UPDATE drafts SET file_context = ? WHERE content_hash = ?",
            params![b, hash],
        )?;
    }
    Ok(())
}

/// Mirrors `EnrichDraftChangedFiles`.
pub fn enrich_draft_changed_files(content_hash: &str, changed_files: &[String]) -> Result<()> {
    if changed_files.is_empty() {
        return Ok(());
    }
    let conn = open();
    let fc_json: Option<String> = conn
        .query_row(
            "SELECT file_context FROM drafts WHERE content_hash = ?",
            params![content_hash],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(None);
    let mut fc: serde_json::Map<String, Value> = serde_json::Map::new();
    if let Some(s) = fc_json {
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&s) {
            fc = m;
        }
    }
    if let Some(existing) = fc.get("changed_files") {
        if let Some(arr) = existing.as_array() {
            if !arr.is_empty() {
                return Ok(());
            }
        }
    }
    fc.insert(
        "changed_files".into(),
        Value::Array(
            changed_files
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    let b = serde_json::to_string(&fc).unwrap_or_else(|_| "{}".into());
    conn.execute(
        "UPDATE drafts SET file_context = ? WHERE content_hash = ?",
        params![b, content_hash],
    )?;
    Ok(())
}

/// Mirrors `GetBundledDraftIDsForProject`.
pub fn get_bundled_draft_ids_for_project(
    project_id: &str,
) -> Result<std::collections::HashSet<String>> {
    if project_id.is_empty() {
        return Ok(Default::default());
    }
    let conn = open();
    let mut stmt = conn.prepare(
        r#"SELECT DISTINCT pci.draft_id
           FROM prompt_commit_items pci
           JOIN prompt_commits pc ON pc.id = pci.prompt_commit_id
           WHERE pc.project_id = ? AND pc.pushed_at IS NULL"#,
    )?;
    let ids = stmt
        .query_map(params![project_id], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}

pub fn update_draft_response(
    session_id: &str,
    prompt_text: &str,
    response_text: &str,
) -> Result<()> {
    if response_text.is_empty() {
        return Ok(());
    }
    let conn = open();
    conn.execute(
        "UPDATE drafts SET response_text = ? WHERE session_id = ? AND prompt_text = ? AND status = 'draft' AND (response_text IS NULL OR LENGTH(response_text) < LENGTH(?))",
        params![response_text, session_id, prompt_text, response_text],
    )?;
    Ok(())
}

pub fn update_draft_tool_calls(
    session_id: &str,
    prompt_text: &str,
    tool_calls: &[Value],
) -> Result<()> {
    if tool_calls.is_empty() {
        return Ok(());
    }
    let conn = open();
    let b = serde_json::to_string(tool_calls)?;
    conn.execute(
        "UPDATE drafts SET tool_calls = ? WHERE session_id = ? AND prompt_text = ? AND status = 'draft'",
        params![b, session_id, prompt_text],
    )?;
    Ok(())
}

pub fn merge_draft_file_context(
    session_id: &str,
    prompt_text: &str,
    updates: &serde_json::Map<String, Value>,
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let conn = open();
    let fc_json: Option<String> = conn
        .query_row(
            "SELECT file_context FROM drafts WHERE session_id = ? AND prompt_text = ? AND status = 'draft'",
            params![session_id, prompt_text],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(None);
    let mut current: serde_json::Map<String, Value> = serde_json::Map::new();
    if let Some(s) = fc_json {
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&s) {
            current = m;
        }
    }
    for (k, v) in updates {
        current.insert(k.clone(), v.clone());
    }
    let b = serde_json::to_string(&current)?;
    conn.execute(
        "UPDATE drafts SET file_context = ? WHERE session_id = ? AND prompt_text = ? AND status = 'draft'",
        params![b, session_id, prompt_text],
    )?;
    Ok(())
}

pub fn update_draft_git_diff(
    session_id: &str,
    prompt_text: &str,
    git_diff: &str,
    head_sha: &str,
) -> Result<()> {
    if git_diff.is_empty() {
        return Ok(());
    }
    let conn = open();
    conn.execute(
        "UPDATE drafts SET git_diff = ?, head_sha = COALESCE(NULLIF(head_sha,''), ?) WHERE session_id = ? AND prompt_text = ? AND status = 'draft' AND (git_diff IS NULL OR git_diff = '')",
        params![git_diff, head_sha, session_id, prompt_text],
    )?;
    Ok(())
}

fn build_where_for_status(
    status: DraftStatus,
    project_ids: &[String],
    project_names: &[String],
) -> (String, Vec<String>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<String> = vec![status.as_str().to_string()];
    let mut where_clause = String::from("status = ?");
    for id in project_ids {
        clauses.push("project_id = ?".into());
        args.push(id.clone());
    }
    for name in project_names {
        clauses.push("project_name = ?".into());
        args.push(name.clone());
    }
    if !clauses.is_empty() {
        clauses.push("(COALESCE(project_id, '') = '' AND COALESCE(project_name, '') = '')".into());
        where_clause.push_str(&format!(" AND ({})", clauses.join(" OR ")));
    }
    (where_clause, args)
}

pub fn get_drafts_by_status(
    status: DraftStatus,
    project_ids: &[String],
    project_names: &[String],
) -> Result<Vec<DraftRecord>> {
    let (where_clause, args) = build_where_for_status(status, project_ids, project_names);
    let sql = format!("SELECT * FROM drafts WHERE {where_clause} ORDER BY captured_at ASC");
    let conn = open();
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), scan_one_draft)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn get_staged_drafts() -> Result<Vec<DraftRecord>> {
    get_drafts_by_status(DraftStatus::Staged, &[], &[])
}

pub fn clear_staged() -> Result<()> {
    let conn = open();
    conn.execute(
        "UPDATE drafts SET status = 'draft' WHERE status = 'staged'",
        [],
    )?;
    Ok(())
}

pub fn stage_drafts(ids: &[String]) -> Result<()> {
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        tx.execute(
            "UPDATE drafts SET status = 'staged' WHERE id = ? AND status = 'draft'",
            params![id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_drafts(ids: &[String]) -> Result<()> {
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        tx.execute(
            "DELETE FROM drafts WHERE id = ? AND status IN ('draft', 'staged')",
            params![id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Scan a single `SELECT * FROM drafts` row into a `DraftRecord`. Column order
/// matches the v1 schema plus the ALTER TABLE appends in v2/v4/v6.
pub(crate) fn scan_one_draft(row: &rusqlite::Row<'_>) -> rusqlite::Result<DraftRecord> {
    let tool_calls_json: Option<String> = row.get("tool_calls")?;
    let file_context_json: Option<String> = row.get("file_context")?;
    let session_commit_shas_json: Option<String> = row.get("session_commit_shas")?;

    let tool_calls: Vec<Value> = tool_calls_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
        .unwrap_or_default();
    let file_context: Option<serde_json::Map<String, Value>> = file_context_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        });
    let session_commit_shas: Vec<String> = session_commit_shas_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();

    Ok(DraftRecord {
        id: row.get("id")?,
        content_hash: row.get("content_hash")?,
        session_id: row.get("session_id")?,
        project_id: row
            .get::<_, Option<String>>("project_id")?
            .unwrap_or_default(),
        project_name: row.get("project_name")?,
        branch_name: row
            .get::<_, Option<String>>("branch_name")?
            .unwrap_or_default(),
        prompt_text: row.get("prompt_text")?,
        response_text: row
            .get::<_, Option<String>>("response_text")?
            .unwrap_or_default(),
        model: row.get::<_, Option<String>>("model")?.unwrap_or_default(),
        source: row.get("source")?,
        capture_method: row.get("capture_method")?,
        tool_calls,
        file_context,
        captured_at: row.get("captured_at")?,
        session_commit_shas,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        git_diff: row
            .get::<_, Option<String>>("git_diff")?
            .unwrap_or_default(),
        head_sha: row
            .get::<_, Option<String>>("head_sha")?
            .unwrap_or_default(),
        permission_mode: row
            .get::<_, Option<String>>("permission_mode")?
            .unwrap_or_default(),
    })
}
