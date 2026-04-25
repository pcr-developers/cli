//! Process-wide event sink. When set, every `display::print_*` writes to
//! the sink instead of stderr.
//!
//! The TUI installs a sink via [`install_sink`] before entering the
//! alternate screen and removes it via [`take_sink`] on exit. Because the
//! sink is a `OnceLock<Mutex<Option<Sender>>>`, install/uninstall is
//! atomic and cheap; the hot-path check ([`with_sink`]) takes a lock just
//! long enough to clone-and-send.

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use super::events::DisplayEvent;

static SINK: OnceLock<Mutex<Option<Sender<DisplayEvent>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Sender<DisplayEvent>>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Replace the current sink (if any) with `tx`. Called by the TUI just
/// before entering raw mode / the alternate screen.
pub fn install_sink(tx: Sender<DisplayEvent>) {
    if let Ok(mut guard) = slot().lock() {
        *guard = Some(tx);
    }
}

/// Remove the current sink, returning it to the caller (so they can drain
/// any pending events). Called by the TUI on graceful exit.
pub fn take_sink() -> Option<Sender<DisplayEvent>> {
    slot().lock().ok().and_then(|mut g| g.take())
}

/// True when a sink is currently installed.
pub fn sink_active() -> bool {
    slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

/// If a sink is installed, run `f` against a clone of the sender and return
/// `true`. If no sink is installed, return `false` so the caller falls
/// through to the line-mode stderr path.
///
/// The `Sender` is cloned per call (cheap — it's just an `Arc` bump) so the
/// closure can take ownership and call `.send()`.
pub fn with_sink<F: FnOnce(&Sender<DisplayEvent>)>(f: F) -> bool {
    let Ok(guard) = slot().lock() else {
        return false;
    };
    let Some(tx) = guard.as_ref() else {
        return false;
    };
    f(tx);
    true
}
