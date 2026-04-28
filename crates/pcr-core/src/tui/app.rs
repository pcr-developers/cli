//! Terminal lifecycle helpers. Every TUI screen pairs `setup_terminal()`
//! at entry with `restore_terminal()` on exit (plus a panic hook so
//! unexpected panics still restore cooked mode). Matches ratatui's
//! canonical init/restore idiom.
//!
//! The cross-screen `pcr start ↔ show ↔ bundle` Tab cycle nests these
//! calls — when one screen exits and the dispatcher launches the next,
//! we must NOT briefly drop back to the main screen buffer (on Windows
//! that flashes the cooked PowerShell prompt under the TUI). To
//! prevent that, both functions refcount entry/exit; only the
//! outermost pair actually enters / leaves the alternate screen.

use std::io::{self, Stderr};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

pub type Term = Terminal<CrosstermBackend<Stderr>>;

/// Number of currently-live TUI screens. Increments on each
/// [`setup_terminal`], decrements on each [`restore_terminal`]. Only
/// the 0→1 transition enters the alt screen; only the 1→0 transition
/// leaves it.
static TUI_DEPTH: AtomicUsize = AtomicUsize::new(0);

pub fn setup_terminal() -> Result<Term> {
    let prev = TUI_DEPTH.fetch_add(1, Ordering::SeqCst);
    if prev == 0 {
        enable_raw_mode()?;
        let mut stderr = io::stderr();
        stderr.execute(EnterAlternateScreen)?;
    }
    let backend = CrosstermBackend::new(io::stderr());
    let mut term = Terminal::new(backend)?;
    // Force a full repaint when a nested screen takes over so we don't
    // inherit the previous screen's framebuffer cells (which cause
    // visible artifacts on the borders / status line).
    term.clear()?;
    install_panic_hook();
    Ok(term)
}

pub fn restore_terminal() -> Result<()> {
    let prev = TUI_DEPTH.fetch_sub(1, Ordering::SeqCst);
    // Guard against an over-decrement from a buggy double-restore;
    // saturate at zero rather than wrap to usize::MAX.
    if prev == 0 {
        TUI_DEPTH.store(0, Ordering::SeqCst);
        return Ok(());
    }
    if prev == 1 {
        let mut stderr = io::stderr();
        let _ = disable_raw_mode();
        stderr.execute(LeaveAlternateScreen)?;
    }
    Ok(())
}

fn install_panic_hook() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // On panic, force-leave the alt screen regardless of depth.
            TUI_DEPTH.store(0, Ordering::SeqCst);
            let mut stderr = io::stderr();
            let _ = disable_raw_mode();
            let _ = stderr.execute(LeaveAlternateScreen);
            prev(info);
        }));
    });
}

/// RAII guard that pins the alt screen open across multiple nested
/// `setup_terminal` / `restore_terminal` calls. Used by the cross-
/// screen Tab cycle in `commands::start::run_tui_cycle` so transitions
/// between Start ↔ Drafts ↔ Bundles never flash the cooked terminal
/// buffer between screens.
pub struct AltScreenGuard {
    _private: (),
}

impl AltScreenGuard {
    pub fn enter() -> Self {
        let prev = TUI_DEPTH.fetch_add(1, Ordering::SeqCst);
        if prev == 0 {
            let _ = enable_raw_mode();
            let mut stderr = io::stderr();
            let _ = stderr.execute(EnterAlternateScreen);
        }
        install_panic_hook();
        Self { _private: () }
    }
}

impl Drop for AltScreenGuard {
    fn drop(&mut self) {
        let prev = TUI_DEPTH.fetch_sub(1, Ordering::SeqCst);
        if prev == 0 {
            TUI_DEPTH.store(0, Ordering::SeqCst);
            return;
        }
        if prev == 1 {
            let mut stderr = io::stderr();
            let _ = disable_raw_mode();
            let _ = stderr.execute(LeaveAlternateScreen);
        }
    }
}
