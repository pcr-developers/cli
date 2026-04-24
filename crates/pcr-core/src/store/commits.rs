//! Bundle / commit CRUD. Mirrors `cli/internal/store/commits.go`.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::db::{null_if_empty, open};
use crate::store::drafts::{scan_one_draft, DraftRecord};
use crate::util::id::new_uuid;
use crate::util::time::now_rfc3339;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptCommit {
    pub id: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_shas: Vec<String>,
    pub head_sha: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pushed_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_id: String,
    pub committed_at: String,
    pub bundle_status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<DraftRecord>,
}

pub fn create_commit(
    message: &str,
    head_sha: &str,
    draft_ids: &[String],
    project_id: &str,
    project_name: &str,
    branch_name: &str,
    bundle_status: &str,
    soft_bundle: bool,
) -> Result<PromptCommit> {
    let id = new_uuid();
    let now = now_rfc3339();
    let bundle_status = if bundle_status.is_empty() {
        "open"
    } else {
        bundle_status
    };

    let mut sha_set = std::collections::BTreeSet::<String>::new();
    {
        let conn = open();
        for draft_id in draft_ids {
            let json: Option<String> = conn
                .query_row(
                    "SELECT session_commit_shas FROM drafts WHERE id = ?",
                    params![draft_id],
                    |r| r.get(0),
                )
                .optional()
                .unwrap_or(None)
                .flatten();
            if let Some(s) = json {
                if let Ok(v) = serde_json::from_str::<Vec<String>>(&s) {
                    for sha in v {
                        sha_set.insert(sha);
                    }
                }
            }
        }
    }
    let session_shas: Vec<String> = sha_set.into_iter().collect();
    let session_shas_json: Option<String> = if session_shas.is_empty() {
        None
    } else {
        serde_json::to_string(&session_shas).ok()
    };

    let conn = open();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        r#"INSERT INTO prompt_commits
            (id, message, project_id, project_name, branch_name, session_shas, head_sha, committed_at, bundle_status)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        params![
            id,
            message,
            null_if_empty(project_id),
            null_if_empty(project_name),
            null_if_empty(branch_name),
            session_shas_json,
            head_sha,
            now,
            bundle_status,
        ],
    )?;
    for draft_id in draft_ids {
        tx.execute(
            "INSERT OR IGNORE INTO prompt_commit_items (prompt_commit_id, draft_id) VALUES (?, ?)",
            params![id, draft_id],
        )?;
        if !soft_bundle {
            tx.execute(
                "UPDATE drafts SET status = 'committed' WHERE id = ?",
                params![draft_id],
            )?;
        }
    }
    tx.commit()?;

    Ok(PromptCommit {
        id,
        message: message.to_string(),
        project_id: project_id.to_string(),
        project_name: project_name.to_string(),
        branch_name: branch_name.to_string(),
        session_shas,
        head_sha: head_sha.to_string(),
        committed_at: now,
        bundle_status: bundle_status.to_string(),
        items: Vec::new(),
        pushed_at: String::new(),
        remote_id: String::new(),
    })
}

pub fn get_open_bundles() -> Result<Vec<PromptCommit>> {
    let conn = open();
    let mut stmt = conn.prepare(
        "SELECT * FROM prompt_commits WHERE bundle_status = 'open' ORDER BY committed_at DESC",
    )?;
    let rows = stmt.query_map([], scan_commit_row)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn get_bundle_by_name(name: &str) -> Result<Option<PromptCommit>> {
    let conn = open();
    let mut stmt = conn.prepare(
        "SELECT * FROM prompt_commits WHERE pushed_at IS NULL AND lower(message) = lower(?) ORDER BY committed_at DESC LIMIT 1",
    )?;
    let rows: Vec<PromptCommit> = stmt
        .query_map(params![name], scan_commit_row)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows.into_iter().next())
}

pub fn rename_bundle(bundle_id: &str, new_name: &str) -> Result<()> {
    let conn = open();
    conn.execute(
        "UPDATE prompt_commits SET message = ? WHERE id = ?",
        params![new_name, bundle_id],
    )?;
    Ok(())
}

pub fn get_open_bundle_by_name(name: &str) -> Result<Option<PromptCommit>> {
    let conn = open();
    let mut stmt = conn.prepare(
        "SELECT * FROM prompt_commits WHERE bundle_status = 'open' AND lower(message) = lower(?) ORDER BY committed_at DESC LIMIT 1",
    )?;
    let rows: Vec<PromptCommit> = stmt
        .query_map(params![name], scan_commit_row)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows.into_iter().next())
}

pub fn remove_drafts_from_bundle(bundle_id: &str, draft_ids: &[String]) -> Result<()> {
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for did in draft_ids {
        tx.execute(
            "DELETE FROM prompt_commit_items WHERE prompt_commit_id = ? AND draft_id = ?",
            params![bundle_id, did],
        )?;
        tx.execute(
            "UPDATE drafts SET status = 'draft' WHERE id = ?",
            params![did],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn add_drafts_to_bundle(bundle_id: &str, draft_ids: &[String], soft: bool) -> Result<()> {
    let conn = open();
    conn.execute(
        "UPDATE prompt_commits SET bundle_status = 'open' WHERE id = ? AND bundle_status = 'closed'",
        params![bundle_id],
    )?;
    let tx = conn.unchecked_transaction()?;
    for did in draft_ids {
        tx.execute(
            "INSERT OR IGNORE INTO prompt_commit_items (prompt_commit_id, draft_id) VALUES (?, ?)",
            params![bundle_id, did],
        )?;
        if !soft {
            tx.execute(
                "UPDATE drafts SET status = 'committed' WHERE id = ?",
                params![did],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_bundle(bundle_id: &str) -> Result<()> {
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        r#"UPDATE drafts SET status = 'draft'
           WHERE id IN (SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?)
             AND status = 'committed'"#,
        params![bundle_id],
    )?;
    tx.execute(
        "DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?",
        params![bundle_id],
    )?;
    tx.execute(
        "DELETE FROM prompt_commits WHERE id = ? AND pushed_at IS NULL",
        params![bundle_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn close_bundle(bundle_id: &str) -> Result<()> {
    let conn = open();
    conn.execute(
        "UPDATE prompt_commits SET bundle_status = 'closed' WHERE id = ?",
        params![bundle_id],
    )?;
    Ok(())
}

pub fn count_unbundled_drafts() -> Result<i64> {
    let conn = open();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM drafts WHERE status IN ('draft', 'staged')",
        [],
        |r| r.get(0),
    )?;
    Ok(n)
}

pub fn list_commits(
    pushed: Option<bool>,
    project_ids: &[String],
    project_names: &[String],
) -> Result<Vec<PromptCommit>> {
    let mut conditions: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if let Some(p) = pushed {
        if p {
            conditions.push("pushed_at IS NOT NULL".into());
        } else {
            conditions.push("pushed_at IS NULL".into());
        }
    }
    let mut project_clauses: Vec<String> = Vec::new();
    for id in project_ids {
        project_clauses.push("project_id = ?".into());
        args.push(id.clone());
    }
    for name in project_names {
        project_clauses.push("project_name = ?".into());
        args.push(name.clone());
    }
    if !project_clauses.is_empty() {
        conditions.push(format!("({})", project_clauses.join(" OR ")));
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    let sql = format!("SELECT * FROM prompt_commits {where_clause} ORDER BY committed_at DESC");
    let conn = open();
    let mut stmt = conn.prepare(&sql)?;
    let bound: Vec<&dyn rusqlite::ToSql> = args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(bound.as_slice(), scan_commit_row)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn get_unpushed_commits() -> Result<Vec<PromptCommit>> {
    list_commits(Some(false), &[], &[])
}

pub fn list_pushed_commits() -> Result<Vec<PromptCommit>> {
    list_commits(Some(true), &[], &[])
}

pub fn unmark_pushed(commit_id: &str) -> Result<()> {
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE prompt_commits SET pushed_at = NULL, remote_id = NULL WHERE id = ?",
        params![commit_id],
    )?;
    tx.execute(
        r#"UPDATE drafts SET status = 'committed'
           WHERE id IN (SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?)
             AND status = 'pushed'"#,
        params![commit_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn get_commit_with_items(commit_id: &str) -> Result<Option<PromptCommit>> {
    let conn = open();
    let mut commit = {
        let mut stmt = conn.prepare("SELECT * FROM prompt_commits WHERE id = ?")?;
        let rows: Vec<PromptCommit> = stmt
            .query_map(params![commit_id], scan_commit_row)?
            .filter_map(|r| r.ok())
            .collect();
        match rows.into_iter().next() {
            Some(c) => c,
            None => return Ok(None),
        }
    };

    let mut items_stmt = conn.prepare(
        r#"SELECT d.* FROM drafts d
           JOIN prompt_commit_items i ON i.draft_id = d.id
           WHERE i.prompt_commit_id = ?
           ORDER BY d.captured_at ASC"#,
    )?;
    let items = items_stmt
        .query_map(params![commit_id], scan_one_draft)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    commit.items = items;
    Ok(Some(commit))
}

pub fn mark_pushed(commit_id: &str, remote_id: &str) -> Result<()> {
    let conn = open();
    let now = now_rfc3339();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE prompt_commits SET pushed_at = ?, remote_id = ? WHERE id = ?",
        params![now, remote_id, commit_id],
    )?;
    tx.execute(
        r#"UPDATE drafts SET status = 'pushed'
           WHERE id IN (SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?)"#,
        params![commit_id],
    )?;
    tx.commit()?;
    Ok(())
}

fn scan_commit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PromptCommit> {
    let session_shas_json: Option<String> = row.get("session_shas")?;
    let session_shas: Vec<String> = session_shas_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();
    let bundle_status: Option<String> = row.get("bundle_status").ok();
    Ok(PromptCommit {
        id: row.get("id")?,
        message: row.get("message")?,
        project_id: row
            .get::<_, Option<String>>("project_id")?
            .unwrap_or_default(),
        project_name: row
            .get::<_, Option<String>>("project_name")?
            .unwrap_or_default(),
        branch_name: row
            .get::<_, Option<String>>("branch_name")?
            .unwrap_or_default(),
        session_shas,
        head_sha: row.get("head_sha")?,
        pushed_at: row
            .get::<_, Option<String>>("pushed_at")?
            .unwrap_or_default(),
        remote_id: row
            .get::<_, Option<String>>("remote_id")?
            .unwrap_or_default(),
        committed_at: row.get("committed_at")?,
        bundle_status: bundle_status.unwrap_or_else(|| "open".to_string()),
        items: Vec::new(),
    })
}
