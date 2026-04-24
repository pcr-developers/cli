//! Timestamp helpers used across commands.

use chrono::{DateTime, Local, SecondsFormat, Utc};

/// Returns the current UTC time as an RFC3339 string with seconds precision.
/// Matches Go's `time.Now().UTC().Format(time.RFC3339)`.
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parse an ISO-8601 timestamp that may or may not include milliseconds.
/// Matches `cmd/bundle.go::parseDraftTime`.
pub fn parse_draft_time(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3fZ") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    None
}

/// Format as `Jan 2 15:04`. Matches `cmd/bundle.go::formatCapturedAt`.
pub fn format_captured_at(s: &str) -> String {
    let Some(dt) = parse_draft_time(s) else {
        return String::new();
    };
    dt.with_timezone(&Local).format("%b %e %H:%M").to_string()
}

/// Format an RFC3339 timestamp as `2006-01-02 15:04`. Matches `cmd/log.go::fmtTime`.
pub fn fmt_time(iso: &str) -> String {
    if iso.is_empty() {
        return String::new();
    }
    let Ok(t) = DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    t.format("%Y-%m-%d %H:%M").to_string()
}

/// Current local clock formatted as `15:04:05`. Matches `display.go::timestamp`.
pub fn local_hms() -> String {
    Local::now().format("%H:%M:%S").to_string()
}
