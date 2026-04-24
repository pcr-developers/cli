//! Terminal lifecycle helpers. Every TUI screen pairs `setup_terminal()`
//! at entry with `restore_terminal()` on exit (plus a panic hook so
//! unexpected panics still restore cooked mode). Matches ratatui's
//! canonical init/restore idiom.

use std::io::{self, Stderr};

use anyhow::Result;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

pub type Term = Terminal<CrosstermBackend<Stderr>>;

pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    stderr.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr);
    let term = Terminal::new(backend)?;
    install_panic_hook();
    Ok(term)
}

pub fn restore_terminal() -> Result<()> {
    let mut stderr = io::stderr();
    let _ = disable_raw_mode();
    stderr.execute(LeaveAlternateScreen)?;
    Ok(())
}

fn install_panic_hook() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            prev(info);
        }));
    });
}
