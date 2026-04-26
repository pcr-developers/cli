//! Garbage collection of old commits. Mirrors `cli/internal/store/gc.go`.

use anyhow::Result;
use chrono::{Duration, Utc};
use rusqlite::params;
use std::path::Path;
use std::process::Command;

use crate::store::db::open;

fn collect_strings(sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
    let conn = open();
    let mut stmt = conn.prepare(sql)?;
    let out: Vec<String> = stmt
        .query_map(params, |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(out)
}

pub fn gc_pushed(older_than_days: i64) -> Result<i64> {
    let cutoff = (Utc::now() - Duration::days(older_than_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let ids = collect_strings(
        "SELECT id FROM prompt_commits WHERE pushed_at IS NOT NULL AND pushed_at < ?",
        &[&cutoff as &dyn rusqlite::ToSql],
    )?;
    delete_commits(&ids)
}

pub fn gc_all_pushed() -> Result<i64> {
    let ids = collect_strings(
        "SELECT id FROM prompt_commits WHERE pushed_at IS NOT NULL",
        &[],
    )?;
    delete_commits(&ids)
}

pub fn gc_unpushed() -> Result<i64> {
    let ids = collect_strings("SELECT id FROM prompt_commits WHERE pushed_at IS NULL", &[])?;
    delete_commits(&ids)
}

/// Delete every unbundled draft (status = 'draft') across all projects.
/// Returns the number of rows removed. Bundled / pushed prompts are
/// untouched — they only disappear via `gc_unpushed` or `gc_pushed`.
pub fn gc_drafts() -> Result<i64> {
    let conn = open();
    let n = conn.execute("DELETE FROM drafts WHERE status = 'draft'", [])?;
    Ok(n as i64)
}

/// Delete unbundled drafts older than `older_than_days`. Useful when a
/// draft list has accumulated stale prompts from old experiments and
/// the user wants to reclaim the view without losing recent work.
pub fn gc_drafts_older_than(older_than_days: i64) -> Result<i64> {
    let cutoff = (Utc::now() - Duration::days(older_than_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let conn = open();
    let n = conn.execute(
        "DELETE FROM drafts WHERE status = 'draft' AND captured_at < ?",
        params![cutoff],
    )?;
    Ok(n as i64)
}

pub fn gc_orphaned(project_path: &Path) -> Result<i64> {
    let rows: Vec<(String, String)> = {
        let conn = open();
        let mut stmt =
            conn.prepare("SELECT id, head_sha FROM prompt_commits WHERE pushed_at IS NULL")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    let mut orphans: Vec<String> = Vec::new();
    for (id, sha) in rows {
        let ok = Command::new("git")
            .arg("cat-file")
            .arg("-e")
            .arg(&sha)
            .current_dir(project_path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            orphans.push(id);
        }
    }
    if orphans.is_empty() {
        return Ok(0);
    }

    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for id in &orphans {
        let draft_ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?")?;
            let ids: Vec<String> = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };
        tx.execute(
            "DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?",
            params![id],
        )?;
        for did in draft_ids {
            tx.execute(
                "UPDATE drafts SET status = 'draft' WHERE id = ? AND status = 'committed'",
                params![did],
            )?;
        }
        tx.execute("DELETE FROM prompt_commits WHERE id = ?", params![id])?;
    }
    tx.commit()?;
    Ok(orphans.len() as i64)
}

fn delete_commits(ids: &[String]) -> Result<i64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut deleted: i64 = 0;
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        let n: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM prompt_commit_items WHERE prompt_commit_id = ?",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        deleted += n;

        let draft_ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?")?;
            let ids: Vec<String> = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };
        tx.execute(
            "DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?",
            params![id],
        )?;
        for did in draft_ids {
            tx.execute("DELETE FROM drafts WHERE id = ?", params![did])?;
        }
        tx.execute("DELETE FROM prompt_commits WHERE id = ?", params![id])?;
    }
    tx.commit()?;
    Ok(deleted)
}

/// Candidate scoring — draft relevance to a list of changed files. Mirrors
/// `cli/internal/store/drafts.go::GetCandidatesForCommit`. Placed here to
/// keep the drafts module from sprawling; it consumes only public drafts API.
pub fn get_candidates_for_commit(
    project_ids: &[String],
    project_names: &[String],
    changed_files: &[String],
) -> Result<(
    Vec<crate::store::DraftRecord>,
    Vec<crate::store::DraftRecord>,
)> {
    use crate::store::{get_drafts_by_status, DraftStatus};
    let drafts = get_drafts_by_status(DraftStatus::Draft, project_ids, project_names)?;
    if changed_files.is_empty() {
        return Ok((Vec::new(), drafts));
    }
    let mut scored: Vec<(crate::store::DraftRecord, i64)> = Vec::with_capacity(drafts.len());
    for d in drafts {
        let mut match_count: i64 = 0;
        let mut touched: std::collections::HashSet<String> = Default::default();
        for tc in &d.tool_calls {
            let mut path = String::new();
            if let Some(input) = tc.get("input").and_then(|v| v.as_object()) {
                path = input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .or_else(|| input.get("file_path").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
            }
            if path.is_empty() {
                if let Some(p) = tc.get("path").and_then(|v| v.as_str()) {
                    path = p.to_string();
                }
            }
            if path.is_empty() {
                continue;
            }
            for cf in changed_files {
                if touched.contains(cf) {
                    continue;
                }
                if path.ends_with(cf.as_str())
                    || cf.ends_with(path.as_str())
                    || path.contains(cf.as_str())
                    || cf.contains(path.as_str())
                {
                    match_count += 1;
                    touched.insert(cf.clone());
                }
            }
        }

        let text = format!("{} {}", d.prompt_text, d.response_text);
        for cf in changed_files {
            if touched.contains(cf) {
                continue;
            }
            let base = std::path::Path::new(cf)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(cf);
            let stem = std::path::Path::new(cf)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(base);
            if text.contains(cf) || text.contains(base) || (stem.len() >= 5 && text.contains(stem))
            {
                match_count += 1;
                touched.insert(cf.clone());
            }
        }
        scored.push((d, match_count));
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    let mut relevant: Vec<crate::store::DraftRecord> = Vec::new();
    let mut unrelated: Vec<crate::store::DraftRecord> = Vec::new();
    for (d, n) in scored {
        if n > 0 {
            relevant.push(d);
        } else {
            unrelated.push(d);
        }
    }
    Ok((relevant, unrelated))
}
