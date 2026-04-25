//! Structured events emitted by [`crate::display`] when a TUI sink is
//! installed. The TUI's main loop drains these and renders them into the
//! appropriate widget — none of this content ever touches stderr.

/// Status of a single capture source as reported by its watcher thread.
/// Renders as a row in the `pcr start` dashboard's WATCHERS panel.
#[derive(Debug, Clone)]
pub enum SourceState {
    /// Watcher thread spawned but hasn't reached its `start` body yet.
    Initializing,
    /// Watcher is healthy and watching `dir`.
    Ready { dir: String },
    /// The directory the watcher targets doesn't exist (e.g. user hasn't
    /// installed the corresponding tool, or the tool stores transcripts
    /// somewhere else on this OS). Watcher remains alive and will activate
    /// if the directory appears.
    Missing { dir: String },
    /// Watcher hit an unrecoverable error and stopped.
    Errored { msg: String },
}

impl SourceState {
    /// Single-word label suitable for dashboard rendering.
    pub fn label(&self) -> &'static str {
        match self {
            SourceState::Initializing => "starting",
            SourceState::Ready { .. } => "ready",
            SourceState::Missing { .. } => "waiting",
            SourceState::Errored { .. } => "error",
        }
    }
}

/// Anything the display module would have written to stderr in line mode
/// becomes one of these variants when a TUI sink is active.
#[derive(Debug, Clone)]
pub enum DisplayEvent {
    /// `pcr start` startup banner — TUI renders inside its own header.
    Banner {
        version: String,
        build_time: String,
        project_count: usize,
    },
    /// A watcher reported a new lifecycle state. The TUI dashboard
    /// maintains a `Map<source, SourceState>` and re-renders the watcher
    /// table on each receipt.
    SourceState { source: String, state: SourceState },
    /// A draft was synced to the server (logged in user).
    Captured {
        project_name: String,
        branch: String,
        model: String,
        prompt_text: String,
        tool_summary: String,
        input_tokens: u64,
        output_tokens: u64,
        exchange_count: u64,
        project_url: String,
        timestamp: String,
    },
    /// A draft was saved locally only (anonymous user).
    Drafted {
        project_name: String,
        branch: String,
        prompt_text: String,
        exchange_count: u64,
        timestamp: String,
    },
    /// Verbose-only event from a watcher (`pcr start --verbose`).
    Verbose {
        source: String,
        msg: String,
        timestamp: String,
    },
    /// User-facing warning. Always rendered, even when verbose is off.
    Error {
        context: String,
        msg: String,
        timestamp: String,
    },
    /// Coaching hint — typically follows an error or empty state.
    Hint { msg: String },
    /// Unstructured line that doesn't fit any other variant. Try to use a
    /// more specific variant before reaching for this.
    Line { msg: String },
}

impl DisplayEvent {
    /// One-line summary suitable for the events log.
    pub fn one_line(&self) -> String {
        match self {
            DisplayEvent::Banner { version, .. } => format!("PCR.dev v{version} started"),
            DisplayEvent::SourceState { source, state } => {
                format!("{source}: {}", state.label())
            }
            DisplayEvent::Captured {
                project_name,
                prompt_text,
                timestamp,
                ..
            } => format!("{timestamp}  {project_name}  ✓  {}", clip(prompt_text, 60)),
            DisplayEvent::Drafted {
                project_name,
                prompt_text,
                timestamp,
                ..
            } => format!("{timestamp}  {project_name}  ◎  {}", clip(prompt_text, 60)),
            DisplayEvent::Verbose {
                source,
                msg,
                timestamp,
            } => format!("{timestamp}  {source}  {msg}"),
            DisplayEvent::Error {
                context,
                msg,
                timestamp,
            } => format!("{timestamp}  ⚠  {context}: {msg}"),
            DisplayEvent::Hint { msg } => format!("→ {msg}"),
            DisplayEvent::Line { msg } => msg.clone(),
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let cut: String = chars.iter().take(take).collect();
    format!("{}…", cut.trim_end())
}
