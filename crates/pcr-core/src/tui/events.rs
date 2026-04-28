//! Unified event stream for ratatui screens.
//!
//! Three input sources merge into one [`Receiver<Event>`]:
//!
//! 1. **Crossterm key events** from a poll thread — user keypresses.
//! 2. **Tick** at a fixed cadence so screens can repaint clocks, animate
//!    spinners, refresh per-second counters, etc.
//! 3. **Display events** from the global [`crate::display`] sink — every
//!    `display::print_*` call from a watcher thread shows up here as a
//!    structured [`DisplayEvent`] instead of raw ANSI on stderr.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, KeyEvent, KeyEventKind};

use crate::display::{install_sink, take_sink, DisplayEvent};

/// Anything the TUI's main loop reacts to.
#[derive(Debug, Clone)]
pub enum Event {
    /// Keyboard input.
    Key(KeyEvent),
    /// Periodic redraw nudge.
    Tick(Duration),
    /// Structured display event from a watcher (or any other module that
    /// called a `display::print_*` while the sink is installed).
    Display(DisplayEvent),
}

/// Owns the merged event channel and keeps its background threads alive
/// for the lifetime of the [`EventSource`]. Drops install/uninstall the
/// display sink so screens compose cleanly.
pub struct EventSource {
    pub rx: Receiver<Event>,
    _tx: Sender<Event>,
}

impl EventSource {
    /// Spawn the keyboard poller, tick generator, and display-sink pump.
    pub fn spawn(tick: Duration) -> Self {
        let (tx, rx) = mpsc::channel();

        // ── Keyboard ───────────────────────────────────────────────────
        // On Windows, crossterm reports both `Press` and `Release` for
        // every keystroke. Forwarding both means handlers fire twice —
        // arrow keys jump 2 rows, single 'q' presses register as two
        // quits, etc. Filter to `Press` (and `Repeat` for held keys)
        // so Linux/macOS — which only ever emit `Press` — behave
        // identically.
        let tx_keys = tx.clone();
        thread::spawn(move || loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(event::Event::Key(k)) = event::read() {
                    if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    if tx_keys.send(Event::Key(k)).is_err() {
                        return;
                    }
                }
            }
        });

        // ── Tick ───────────────────────────────────────────────────────
        let tx_tick = tx.clone();
        thread::spawn(move || loop {
            thread::sleep(tick);
            if tx_tick.send(Event::Tick(tick)).is_err() {
                return;
            }
        });

        // ── Display sink ──────────────────────────────────────────────
        // Install a fresh channel into the global display sink. A pump
        // thread forwards each DisplayEvent into the unified Event stream.
        let (display_tx, display_rx) = mpsc::channel::<DisplayEvent>();
        install_sink(display_tx);
        let tx_display = tx.clone();
        thread::spawn(move || {
            while let Ok(ev) = display_rx.recv() {
                if tx_display.send(Event::Display(ev)).is_err() {
                    return;
                }
            }
        });

        Self { rx, _tx: tx }
    }
}

impl Drop for EventSource {
    fn drop(&mut self) {
        // Detach the global sink so subsequent line-mode writes go to
        // stderr again. The pump thread terminates naturally when the
        // sender it was reading from is dropped (which happens when this
        // returned `Sender` falls out of scope).
        let _ = take_sink();
    }
}
