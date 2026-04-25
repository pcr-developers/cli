//! Claude Code `Stop` hook. Direct port of
//! `cli/internal/sources/claudecode/hook.go`.
//!
//! Called by `pcr hook` after every Claude Code response. Finds any new
//! drafts for the current project, prompts the user via `/dev/tty`, and
//! adds them to an open bundle (or creates a new one).

use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use crate::sources::shared::git::git_output;
use crate::store::{self, DraftRecord, DraftStatus, PromptCommit};
use crate::util::text::plural;

/// Main hook entry. Always returns Ok so Claude Code never sees an error.
pub fn run_hook(
    project_ids: &[String],
    project_names: &[String],
    project_name: &str,
) -> Result<()> {
    // Poll up to 2s for the watcher to process new drafts (1s debounce +
    // some slack for disk I/O).
    let mut drafts: Vec<DraftRecord> = Vec::new();
    for _ in 0..4 {
        drafts = store::get_drafts_by_status(DraftStatus::Draft, project_ids, project_names)
            .unwrap_or_default();
        if !drafts.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if drafts.is_empty() {
        return Ok(());
    }
    let n = drafts.len();

    // Open /dev/tty — Claude Code holds stdin, so we can't read from it.
    #[cfg(unix)]
    let tty_opt = open_tty();
    #[cfg(not(unix))]
    let tty_opt: Option<TtyHandle> = None;

    let Some(mut tty) = tty_opt else {
        return Ok(());
    };

    let open_bundles = store::get_open_bundles().unwrap_or_default();
    let mut target_bundle: Option<PromptCommit> = None;
    let mut bundle_name = git_branch();
    if bundle_name.is_empty() || bundle_name == "HEAD" {
        bundle_name = "untitled".to_string();
    }
    if let Some(b) = open_bundles.first() {
        target_bundle = Some(b.clone());
        bundle_name = b.message.clone();
    }

    let _ = write!(
        tty.writer(),
        "\r\nPCR: {n} new prompt{} — add to {bundle_name:?}? [Y/n/b] ",
        plural(n),
    );
    tty.flush();

    let ch = tty.read_single_char();

    let _ = write!(tty.writer(), "\r\n");
    tty.flush();

    match ch {
        Some('b') | Some('B') => {
            hook_create_new_bundle(&mut tty, &drafts, project_ids, project_name)
        }
        Some('n') | Some('N') => Ok(()),
        // Enter / Y / y — default to add.
        Some('\r') | Some('\n') | Some('Y') | Some('y') => hook_add_to_bundle(
            &mut tty,
            &drafts,
            target_bundle.as_ref(),
            &bundle_name,
            project_ids,
            project_name,
            n,
        ),
        _ => Ok(()),
    }
}

fn hook_add_to_bundle(
    tty: &mut TtyHandle,
    drafts: &[DraftRecord],
    target_bundle: Option<&PromptCommit>,
    bundle_name: &str,
    project_ids: &[String],
    project_name: &str,
    n: usize,
) -> Result<()> {
    let ids = draft_ids(drafts);
    match target_bundle {
        Some(bundle) => {
            if let Err(e) = store::add_drafts_to_bundle(&bundle.id, &ids, true) {
                let _ = writeln!(tty.writer(), "PCR: error: {e}");
                return Ok(());
            }
            let _ = writeln!(
                tty.writer(),
                "PCR: Added {n} prompt{} to {bundle_name:?}",
                plural(n)
            );
        }
        None => {
            let project_id = project_ids.first().cloned().unwrap_or_default();
            let synthetic_sha = format!(
                "hook-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            if let Err(e) = store::create_commit(
                bundle_name,
                &synthetic_sha,
                &ids,
                &project_id,
                project_name,
                bundle_name,
                "open",
                true,
            ) {
                let _ = writeln!(tty.writer(), "PCR: error: {e}");
                return Ok(());
            }
            let _ = writeln!(
                tty.writer(),
                "PCR: Created prompt bundle {bundle_name:?} with {n} prompt{}",
                plural(n)
            );
        }
    }
    Ok(())
}

fn hook_create_new_bundle(
    tty: &mut TtyHandle,
    drafts: &[DraftRecord],
    project_ids: &[String],
    project_name: &str,
) -> Result<()> {
    let _ = write!(tty.writer(), "PCR: New bundle name: ");
    tty.flush();
    let name = tty.read_line();
    if name.is_empty() {
        let _ = writeln!(tty.writer(), "PCR: Cancelled — no name given.");
        return Ok(());
    }
    let project_id = project_ids.first().cloned().unwrap_or_default();
    let ids = draft_ids(drafts);
    let branch = git_branch();
    let synthetic_sha = format!(
        "hook-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    if let Err(e) = store::create_commit(
        &name,
        &synthetic_sha,
        &ids,
        &project_id,
        project_name,
        &branch,
        "open",
        true,
    ) {
        let _ = writeln!(tty.writer(), "PCR: error: {e}");
        return Ok(());
    }
    let _ = writeln!(
        tty.writer(),
        "PCR: Created prompt bundle {name:?} with {} prompt{}",
        drafts.len(),
        plural(drafts.len())
    );
    Ok(())
}

fn git_branch() -> String {
    // Detached-HEAD becomes empty string instead of the literal "HEAD".
    let b = git_output(&["rev-parse", "--abbrev-ref", "HEAD"]);
    if b == "HEAD" {
        String::new()
    } else {
        b
    }
}

fn draft_ids(drafts: &[DraftRecord]) -> Vec<String> {
    drafts.iter().map(|d| d.id.clone()).collect()
}

// ─── /dev/tty helper ─────────────────────────────────────────────────────────

#[cfg(unix)]
pub struct TtyHandle {
    file: std::fs::File,
}

#[cfg(not(unix))]
pub struct TtyHandle;

#[cfg(unix)]
fn open_tty() -> Option<TtyHandle> {
    use std::fs::OpenOptions;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    Some(TtyHandle { file })
}

#[cfg(unix)]
impl TtyHandle {
    fn writer(&mut self) -> &mut std::fs::File {
        &mut self.file
    }
    fn flush(&mut self) {
        let _ = self.file.flush();
    }
    /// Read a single character, consuming any escape sequence that follows
    /// an ESC (0x1b) byte. Mirrors the raw-mode block in the Go hook.
    fn read_single_char(&mut self) -> Option<char> {
        use std::io::Read;
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&self.file);
        let prev = raw_mode::enable(fd).ok();
        let mut buf = [0u8; 1];
        let mut drain = [0u8; 32];
        let result = loop {
            if self.file.read_exact(&mut buf).is_err() {
                break None;
            }
            let b = buf[0];
            if b == 0x1b {
                // Escape sequence — drain and try again.
                let _ = self.file.read(&mut drain);
                continue;
            }
            if b < 0x20 && b != b'\r' && b != b'\n' {
                continue;
            }
            break Some(b as char);
        };
        if let Some(prev) = prev {
            raw_mode::restore(fd, &prev);
        }
        result
    }
    fn read_line(&mut self) -> String {
        let mut reader = BufReader::new(&mut self.file);
        let mut buf = String::new();
        let _ = reader.read_line(&mut buf);
        buf.trim_end_matches(&['\r', '\n'][..]).trim().to_string()
    }
}

#[cfg(not(unix))]
impl TtyHandle {
    fn writer(&mut self) -> std::io::Sink {
        std::io::sink()
    }
    fn flush(&mut self) {}
    fn read_single_char(&mut self) -> Option<char> {
        None
    }
    fn read_line(&mut self) -> String {
        String::new()
    }
}

#[cfg(unix)]
mod raw_mode {
    // Minimal raw-mode toggle using libc via FFI without adding the `libc`
    // crate. We hand-declare the two calls we need.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
        c_iflag: u64,
        c_oflag: u64,
        c_cflag: u64,
        c_lflag: u64,
        // Padding is fine on both macOS and Linux because we always pass
        // the full struct by pointer and never introspect these bytes.
        rest: [u8; 256],
    }

    extern "C" {
        fn tcgetattr(fd: i32, ptr: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, opt: i32, ptr: *const Termios) -> i32;
        fn cfmakeraw(ptr: *mut Termios);
    }

    pub fn enable(fd: i32) -> std::io::Result<Termios> {
        unsafe {
            let mut prev: Termios = std::mem::zeroed();
            if tcgetattr(fd, &mut prev) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut raw = prev;
            cfmakeraw(&mut raw);
            let tcsa_now = 0;
            if tcsetattr(fd, tcsa_now, &raw) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(prev)
        }
    }

    pub fn restore(fd: i32, prev: &Termios) {
        unsafe {
            let tcsa_now = 0;
            let _ = tcsetattr(fd, tcsa_now, prev);
        }
    }
}
