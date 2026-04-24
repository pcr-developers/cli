//! Session state change log. Mirrors `cli/internal/store/session_state_events.go`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

use crate::store::db::open;

#[derive(Debug, Clone, Default)]
pub struct SessionStateEvent {
    pub id: i64,
    pub session_id: String,
    pub occurred_at: DateTime<Utc>,
    pub unified_mode: String,
    pub model_name: String,
    pub context_tokens_used: i64,
    pub context_token_limit: i64,
}

pub fn record_session_state_event(e: &SessionStateEvent) -> Result<()> {
    let conn = open();
    conn.execute(
        r#"INSERT INTO session_state_events
            (session_id, occurred_at, unified_mode, model_name, context_tokens_used, context_token_limit)
           VALUES (?, ?, ?, ?, ?, ?)"#,
        params![
            e.session_id,
            e.occurred_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            e.unified_mode,
            e.model_name,
            e.context_tokens_used,
            e.context_token_limit,
        ],
    )?;
    Ok(())
}

pub fn get_session_state_at(
    session_id: &str,
    at: DateTime<Utc>,
) -> Result<Option<SessionStateEvent>> {
    let conn = open();
    let at_s = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let row = conn
        .query_row(
            r#"SELECT id, session_id, occurred_at, unified_mode, model_name,
                      context_tokens_used, context_token_limit
               FROM session_state_events
               WHERE session_id = ? AND occurred_at <= ?
               ORDER BY occurred_at DESC
               LIMIT 1"#,
            params![session_id, at_s],
            |r| {
                let occurred_s: String = r.get(2)?;
                let occurred = DateTime::parse_from_rfc3339(&occurred_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                Ok(SessionStateEvent {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    occurred_at: occurred,
                    unified_mode: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    model_name: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    context_tokens_used: r.get::<_, Option<i64>>(5)?.unwrap_or_default(),
                    context_token_limit: r.get::<_, Option<i64>>(6)?.unwrap_or_default(),
                })
            },
        )
        .optional()?;
    Ok(row)
}
