//! Event stream used by every TUI screen. Background thread reads
//! `crossterm` keyboard events and forwards them into the main event loop
//! via an `mpsc::Sender` alongside domain events the screen posts itself.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, KeyEvent};

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    /// Tick fires at a fixed cadence so screens can redraw progress, tokens, etc.
    Tick(Duration),
    /// Domain event a screen posts to itself (watcher status, API response, etc).
    Custom(&'static str, String),
}

pub struct EventSource {
    pub rx: Receiver<Event>,
    _tx: Sender<Event>,
}

impl EventSource {
    pub fn spawn(tick: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let tx_keys = tx.clone();
        thread::spawn(move || loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(event::Event::Key(k)) = event::read() {
                    if tx_keys.send(Event::Key(k)).is_err() {
                        return;
                    }
                }
            }
        });
        let tx_tick = tx.clone();
        thread::spawn(move || loop {
            thread::sleep(tick);
            if tx_tick.send(Event::Tick(tick)).is_err() {
                return;
            }
        });
        Self { rx, _tx: tx }
    }
}
