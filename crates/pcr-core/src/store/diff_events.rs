//! Diff event log. Mirrors `cli/internal/store/diff_events.go`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::store::db::open;

#[derive(Debug, Clone)]
pub struct DiffEvent {
    pub id: i64,
    pub project_id: String,
    pub project_name: String,
    pub files: Vec<String>,
    pub occurred_at: DateTime<Utc>,
}

pub fn record_diff_event(
    project_id: &str,
    project_name: &str,
    files: &[String],
    at: DateTime<Utc>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let files_json = serde_json::to_string(files)?;
    let conn = open();
    conn.execute(
        "INSERT INTO diff_events (project_id, project_name, files, occurred_at) VALUES (?, ?, ?, ?)",
        params![
            project_id,
            project_name,
            files_json,
            at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ],
    )?;
    Ok(())
}

pub fn get_diff_events_in_window(
    from: Option<DateTime<Utc>>,
    to: DateTime<Utc>,
) -> Result<Vec<DiffEvent>> {
    let conn = open();
    let to_s = to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (sql, rows) = if let Some(from) = from {
        let from_s = from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = "SELECT id, project_id, project_name, files, occurred_at FROM diff_events WHERE occurred_at > ? AND occurred_at <= ? ORDER BY occurred_at ASC";
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![from_s, to_s], map_diff_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        (sql, rows)
    } else {
        let sql = "SELECT id, project_id, project_name, files, occurred_at FROM diff_events WHERE occurred_at <= ? ORDER BY occurred_at ASC";
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![to_s], map_diff_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        (sql, rows)
    };
    let _ = sql;
    Ok(rows)
}

pub fn prune_diff_events(before: DateTime<Utc>) -> Result<()> {
    let conn = open();
    conn.execute(
        "DELETE FROM diff_events WHERE occurred_at < ?",
        params![before.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)],
    )?;
    Ok(())
}

pub fn delete_diff_events_by_id(ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat("?")
        .take(ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("DELETE FROM diff_events WHERE id IN ({placeholders})");
    let conn = open();
    let bound: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|n| n as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, bound.as_slice())?;
    Ok(())
}

fn map_diff_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiffEvent> {
    let id: i64 = row.get(0)?;
    let project_id: String = row.get(1)?;
    let project_name: String = row.get(2)?;
    let files_json: String = row.get(3)?;
    let occurred_at_s: String = row.get(4)?;
    let files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
    let occurred_at = DateTime::parse_from_rfc3339(&occurred_at_s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    Ok(DiffEvent {
        id,
        project_id,
        project_name,
        files,
        occurred_at,
    })
}
