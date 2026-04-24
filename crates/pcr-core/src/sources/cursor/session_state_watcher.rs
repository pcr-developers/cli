//! Cursor session state watcher. Direct port of
//! `cli/internal/sources/cursor/session_state_watcher.go`.

use chrono::Utc;
use std::collections::HashMap;
use std::time::Duration;

use crate::display;
use crate::sources::cursor::db::all_composer_state_rows;
use crate::store::{self, SessionStateEvent};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SessionSnapshot {
    unified_mode: String,
    model_name: String,
    context_tokens_used: i64,
    context_token_limit: i64,
}

pub struct SessionStateWatcher {
    prev_state: HashMap<String, SessionSnapshot>,
}

impl SessionStateWatcher {
    pub fn new() -> Self {
        Self {
            prev_state: HashMap::new(),
        }
    }

    /// Run the 2-second polling loop. Call in a dedicated thread.
    pub fn run_blocking(mut self) {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            self.poll();
        }
    }

    fn poll(&mut self) {
        let now = Utc::now();
        for row in all_composer_state_rows() {
            let snap = SessionSnapshot {
                unified_mode: row.unified_mode,
                model_name: row.model_name,
                context_tokens_used: row.context_tokens_used,
                context_token_limit: row.context_token_limit,
            };
            let prev = self.prev_state.get(&row.composer_id).cloned();
            if prev.as_ref() == Some(&snap) {
                continue;
            }
            self.prev_state
                .insert(row.composer_id.clone(), snap.clone());
            let _ = store::record_session_state_event(&SessionStateEvent {
                session_id: row.composer_id.clone(),
                occurred_at: now,
                unified_mode: snap.unified_mode.clone(),
                model_name: snap.model_name.clone(),
                context_tokens_used: snap.context_tokens_used,
                context_token_limit: snap.context_token_limit,
                ..Default::default()
            });
            if let Some(prev) = prev {
                let short = &row.composer_id[..row.composer_id.len().min(8)];
                if prev.unified_mode != snap.unified_mode && !snap.unified_mode.is_empty() {
                    display::print_verbose_event(
                        "session",
                        &format!(
                            "[{short}]  mode  {} → {}",
                            prev.unified_mode, snap.unified_mode
                        ),
                    );
                }
                if prev.model_name != snap.model_name && !snap.model_name.is_empty() {
                    display::print_verbose_event(
                        "session",
                        &format!(
                            "[{short}]  model  {} → {}",
                            prev.model_name, snap.model_name
                        ),
                    );
                }
                if prev.context_tokens_used != snap.context_tokens_used
                    && snap.context_token_limit > 0
                {
                    let pct = 100i64 * snap.context_tokens_used / snap.context_token_limit;
                    display::print_verbose_event(
                        "session",
                        &format!(
                            "[{short}]  context  {}/{} ({}%)",
                            snap.context_tokens_used, snap.context_token_limit, pct
                        ),
                    );
                }
            }
        }
    }
}
